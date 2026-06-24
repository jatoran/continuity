//! File command, drag/drop, and banner handling.
//!
//! Methods here show dialogs, route HWND drop messages, and enqueue work
//! to the file-I/O worker. Disk reads/writes happen on `file_io`.

use std::path::PathBuf;

use continuity_buffer::{BufferId, FileAssociation};
use continuity_command::Error as CommandError; // alias: collides with crate::Error
use continuity_render::{FocusField, FooterText, OverlayDraw, PanelStyle, Rect as DrawRect, Rgba}; // alias: `Rect` collides with `crate::pane_layout::Rect`
use continuity_text::{EditOp, Position, Range};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Shell::{DragFinish, DragQueryPoint, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use crate::file_io::FileIoEvent;
use crate::window::{Window, FILE_IO_TIMER_ID, FILE_IO_TIMER_MS};
use crate::window_file_dialogs::{dropped_paths, open_file_dialog, save_file_dialog};
use crate::window_file_image_drop::is_dropped_image_path;

/// Non-blocking file prompt shown above the editor.
#[derive(Clone, Debug)]
pub struct FileBanner {
    text: String,
    pub(crate) pending: Option<PendingExternalChange>,
    /// UNIX-epoch milliseconds at which this banner should auto-dismiss.
    /// `None` means the banner is sticky and only dismisses on user
    /// action (Esc, reload/keep/diff button, etc.) — used for banners
    /// that require a response such as external-disk-change and hard
    /// failures.
    expires_at_ms: Option<u64>,
}

/// Default auto-dismiss for transient status banners ("Saved …",
/// "Reloaded …"). Long enough to read, short enough that the
/// chrome doesn't linger past the action it announces.
const TRANSIENT_BANNER_MS: u64 = 2500;

impl FileBanner {
    /// Sticky banner — stays until explicitly dismissed. Used for
    /// failures and external-change prompts that require user input.
    pub fn new(text: String) -> Self {
        Self {
            text,
            pending: None,
            expires_at_ms: None,
        }
    }

    /// Transient banner — auto-dismisses after [`TRANSIENT_BANNER_MS`]
    /// from `now_ms`. Used for confirm-only status text like "Saved …"
    /// where the action has already completed.
    pub fn transient(text: String, now_ms: u64) -> Self {
        Self::transient_for(text, now_ms, TRANSIENT_BANNER_MS)
    }

    /// Transient banner with a caller-owned display duration.
    pub(crate) fn transient_for(text: String, now_ms: u64, duration_ms: u64) -> Self {
        Self {
            text,
            pending: None,
            expires_at_ms: Some(now_ms.saturating_add(duration_ms)),
        }
    }

    /// `true` when `now_ms` has reached or passed the auto-dismiss
    /// deadline. Sticky banners (`expires_at_ms == None`) always
    /// return `false`.
    #[must_use]
    pub fn is_expired(&self, now_ms: u64) -> bool {
        matches!(self.expires_at_ms, Some(t) if now_ms >= t)
    }

    pub(crate) fn has_deadline(&self) -> bool {
        self.expires_at_ms.is_some()
    }

    /// `true` when the banner text exactly matches `text`.
    #[must_use]
    pub(crate) fn has_text(&self, text: &str) -> bool {
        self.text == text
    }

    fn external(buffer_id: BufferId, path: PathBuf, from_save: bool) -> Self {
        Self {
            text: format!("{} changed on disk:", path.display()),
            pending: Some(PendingExternalChange {
                buffer_id,
                path,
                disk_content: None,
                from_save,
            }),
            expires_at_ms: None,
        }
    }

    /// Build the conflict banner carrying the on-disk content (for the diff
    /// view). `from_save` marks a conflict raised by a refused *save* (vs.
    /// the live watcher) — it makes "keep mine" force-write the editor's
    /// version, since the user was actively trying to persist it.
    pub(crate) fn external_with_content(
        buffer_id: BufferId,
        path: PathBuf,
        disk_content: String,
        _disk_file: FileAssociation,
        from_save: bool,
    ) -> Self {
        let mut banner = Self::external(buffer_id, path, from_save);
        if let Some(pending) = banner.pending.as_mut() {
            pending.disk_content = Some(disk_content);
        }
        banner
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PendingExternalChange {
    pub(crate) buffer_id: BufferId,
    pub(crate) path: PathBuf,
    pub(crate) disk_content: Option<String>,
    /// `true` when this conflict was raised by a refused save (the user was
    /// trying to persist), so "keep mine" writes their version; `false`
    /// for a live watcher change, where "keep mine" only dismisses.
    pub(crate) from_save: bool,
}

impl Window {
    /// Start polling file-I/O worker completions.
    pub(crate) fn start_file_io_poll(&mut self, hwnd: HWND) {
        let has_banner_deadline = self
            .file_banner
            .as_ref()
            .is_some_and(FileBanner::has_deadline);
        if (self.file_io.is_none() && !has_banner_deadline) || self.file_io_poll_active {
            return;
        }
        unsafe {
            let _ = SetTimer(Some(hwnd), FILE_IO_TIMER_ID, FILE_IO_TIMER_MS, None);
        }
        self.file_io_poll_active = true;
    }

    /// Drain file-I/O completion events.
    pub(crate) fn on_file_io_tick(&mut self, hwnd: HWND) {
        let mut changed = false;
        let mut reason = "invalidate_rect";
        if let Some(file_io) = self.file_io.clone() {
            while let Ok(event) = file_io.events().try_recv() {
                if matches!(
                    &event,
                    FileIoEvent::ExternalChanged { .. }
                        | FileIoEvent::Deleted { .. }
                        | FileIoEvent::Reloaded { .. }
                        | FileIoEvent::Rechecked { .. }
                        | FileIoEvent::SaveConflict { .. }
                ) {
                    reason = "external_invalidate";
                }
                self.handle_file_io_event(event);
                changed = true;
            }
        }
        while let Ok(event) = self.file_open_rx.try_recv() {
            self.handle_file_io_event(event);
            changed = true;
        }
        if let Some(banner) = self.file_banner.as_ref() {
            if banner.is_expired(self.now_ms()) {
                self.file_banner = None;
                changed = true;
                reason = "banner_expired";
            }
        }
        if self.file_io.is_none()
            && !self
                .file_banner
                .as_ref()
                .is_some_and(FileBanner::has_deadline)
            && self.file_io_poll_active
        {
            unsafe {
                let _ = KillTimer(Some(hwnd), FILE_IO_TIMER_ID);
            }
            self.file_io_poll_active = false;
        }
        if changed {
            self.invalidate_with_reason(hwnd, reason);
        }
    }

    /// Build a passive banner overlay when no input overlay is active.
    ///
    /// Layout (item [13b]/[13c]): the panel sits *below* the tab ribbon
    /// (`TAB_STRIP_HEIGHT_DIP` + a small gap) so it never overlaps the
    /// tab strip, and the text field is tall enough (`FIELD_HEIGHT_DIP`)
    /// that descenders are not clipped by the `paint_focus_field` inset.
    pub(crate) fn file_banner_overlay(&self, width: f32) -> Option<OverlayDraw> {
        let banner = self.file_banner.as_ref()?;
        // Only the external-change conflict banner (which carries a pending
        // decision) shows clickable action buttons; transient/info banners
        // render as a plain field. Geometry is shared with the click
        // hit-test via `banner_geometry` so painted and clickable rects
        // never drift (see `window_file_banner_buttons`).
        let with_buttons = banner.pending.is_some();
        let geo = self.banner_geometry(width, with_buttons);
        let list_rows: Vec<_> = geo
            .buttons
            .iter()
            .map(crate::window_file_banner_buttons::banner_button_row)
            .collect();
        let field_top = geo.field_rect.y;
        let panel_top = geo.panel_rect.y;
        let panel_height = geo.panel_rect.h;
        let field_left = geo.field_rect.x;
        let field_width = geo.field_rect.w;

        Some(OverlayDraw {
            panel: PanelStyle {
                rect: geo.panel_rect,
                corner_radius: 6.0,
                // Slightly darker fill + brighter, fully opaque accent
                // border than the surrounding chrome so the banner stands
                // out a touch (item [13c]).
                bg: Rgba {
                    r: 0.10,
                    g: 0.11,
                    b: 0.13,
                    a: 0.98,
                },
                border: Rgba {
                    r: 0.55,
                    g: 0.62,
                    b: 0.72,
                    a: 1.0,
                },
                shadow: Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.45,
                },
                shadow_offset: 4.0,
            },
            input_focused: false,
            focus_field: Some(FocusField {
                rect: DrawRect::new(field_left, field_top, field_width, geo.field_rect.h),
                text: banner.text.clone(),
                placeholder: None,
                caret_byte: banner.text.len(),
                selection_range: None,
                fg: Rgba {
                    r: 0.94,
                    g: 0.96,
                    b: 0.99,
                    a: 1.0,
                },
                selection_bg: Rgba::TRANSPARENT,
                placeholder_fg: Rgba::TRANSPARENT,
                caret_color: Rgba::TRANSPARENT,
                focus_ring: Rgba::TRANSPARENT,
            }),
            secondary_field: None,
            list_rows,
            scrollbar: None,
            footer: Some(FooterText {
                rect: DrawRect::new(field_left, panel_top + panel_height - 1.0, 1.0, 1.0),
                text: String::new(),
                fg: Rgba::TRANSPARENT,
            }),
        })
    }

    /// Handle `WM_DROPFILES`.
    pub(crate) fn on_drop_files(&mut self, hdrop_raw: isize) {
        let hdrop = HDROP(hdrop_raw as *mut core::ffi::c_void);
        let mut point = POINT::default();
        let target = unsafe {
            if DragQueryPoint(hdrop, &mut point).as_bool() {
                self.pane_at(point.x as f32, point.y as f32)
            } else {
                Some(self.tree.focused)
            }
        };
        let paths = dropped_paths(hdrop);
        unsafe {
            DragFinish(hdrop);
        }
        if paths.is_empty() {
            return;
        }

        // F5: partition image drops out of the tab-open path. Images
        // route through the hash-deduped shared store and insert a
        // markdown reference at the caret of the drop-target pane.
        // Non-image drops keep the legacy tab-open behaviour.
        let (image_paths, file_paths): (Vec<PathBuf>, Vec<PathBuf>) =
            paths.into_iter().partition(|p| is_dropped_image_path(p));

        if !image_paths.is_empty() {
            self.import_dropped_images(image_paths, target);
        }
        if file_paths.is_empty() {
            return;
        }
        if let Some(pane) = target {
            self.switch_focus(pane);
        }
        if let Err(err) = self.file_open_paths_impl(file_paths) {
            self.file_banner = Some(FileBanner::new(err.to_string()));
        }
    }

    /// Watch any file-associated tabs restored from persistence.
    pub(crate) fn watch_existing_file_tabs(&self) {
        let Some(file_io) = self.file_io.as_ref() else {
            return;
        };
        for tab in self.tree.tabs.values() {
            let Some(snap) = self.editor.snapshot(tab.buffer_id) else {
                continue;
            };
            if let Some(file) = snap.file {
                let _ = file_io.watch_file(tab.buffer_id, file);
            }
        }
    }

    pub(crate) fn file_open_dialog_impl(&mut self) -> Result<(), CommandError> {
        let Some(path) = open_file_dialog(self.hwnd) else {
            return Ok(());
        };
        self.file_open_paths_impl(vec![path])
    }

    pub(crate) fn file_open_paths_impl(&mut self, paths: Vec<PathBuf>) -> Result<(), CommandError> {
        self.file_open_files_or_folder_paths(paths)
    }

    pub(crate) fn file_save_impl(&mut self) -> Result<(), CommandError> {
        self.maybe_trim_trailing_whitespace_for_save();
        let snap = self
            .editor
            .snapshot(self.buffer_id)
            .ok_or(CommandError::UnsupportedContext("file_save"))?;
        let Some(file) = snap.file.as_ref() else {
            return self.file_save_as_impl();
        };
        let path = file.path.clone();
        let base = file.clone();
        let content = snap.rope_snapshot().rope().to_string();
        // Guard against silently overwriting an external change: the worker
        // refuses the write if the on-disk hash no longer matches what we
        // last synced (`file.hash`).
        let expected_hash = Some(file.hash);
        self.mark_saved_clean(self.buffer_id, base, &content);
        self.enqueue_save(self.buffer_id, path, content, expected_hash)
    }

    pub(crate) fn file_save_as_impl(&mut self) -> Result<(), CommandError> {
        self.maybe_trim_trailing_whitespace_for_save();
        let snap = self
            .editor
            .snapshot(self.buffer_id)
            .ok_or(CommandError::UnsupportedContext("file_save_as"))?;
        // Default the save name to the active tab's title (sanitized inside
        // the dialog helper), falling back to "untitled".
        let default_title = self
            .tree
            .active_tab()
            .map(|tab| self.tab_label(tab))
            .unwrap_or_default();
        let Some(path) = save_file_dialog(self.hwnd, snap.file.as_ref(), &default_title) else {
            return Ok(());
        };
        let content = snap.rope_snapshot().rope().to_string();
        let base = FileAssociation::new(path.clone(), 0, 0);
        self.mark_saved_clean(self.buffer_id, base, &content);
        // Save-as is an explicit user choice of target (the dialog already
        // confirms any overwrite), so write unconditionally.
        self.enqueue_save(self.buffer_id, path, content, None)
    }

    fn handle_file_io_event(&mut self, event: FileIoEvent) {
        match event {
            FileIoEvent::Opened {
                target_pane,
                content,
                file,
            } => self.handle_opened_file(target_pane, content, file),
            FileIoEvent::DirectoryListed {
                root,
                relative,
                entries,
                truncated,
            } => self.handle_file_tree_directory_list(root, relative, entries, truncated),
            FileIoEvent::Saved { buffer_id, file } => {
                // Write confirmed — drop the failure-rollback baseline.
                self.pending_save_baseline.remove(&buffer_id);
                let _ = self
                    .editor
                    .set_file_association(buffer_id, Some(file.clone()));
                self.mark_tab_file_associated(buffer_id, &file);
                self.decoration_cache.evict(buffer_id.as_uuid().as_u128());
                self.last_submitted_decoration_revision_per_buffer
                    .borrow_mut()
                    .remove(&buffer_id);
                if buffer_id == self.buffer_id {
                    self.language_revision = None;
                    self.last_submitted_decoration_revision = None;
                    self.refresh_language();
                    self.maybe_submit_decoration();
                }
                if let Some(register_file_buffer) = self.register_file_buffer.as_ref() {
                    register_file_buffer(buffer_id, file.clone());
                }
                let now = self.now_ms();
                self.file_banner = Some(FileBanner::transient(
                    format!("Saved {}", file.path.display()),
                    now,
                ));
                // α.1 save-confirm chip — quick glanceable acknowledgement
                // in the status bar. Real file I/O only; the durable
                // autosave to SQLite stays invisible.
                let file_label = file
                    .path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("file")
                    .to_string();
                crate::window_status_notice::push_save_confirm_notice(
                    &mut self.status_notices,
                    &file_label,
                    now,
                );
                self.start_motion_timer();
                if let Some(file_io) = self.file_io.as_ref() {
                    let _ = file_io.watch_file(buffer_id, file);
                }
            }
            FileIoEvent::SaveConflict {
                buffer_id,
                path: _,
                content,
                file,
            } => {
                // The save was refused — the file changed on disk since we
                // last synced. The optimistic `mark_saved_clean` was wrong
                // (no write happened): roll the content hash back to its
                // pre-save value so the buffer is dirty again, then run the
                // standard reconcile, which raises the reload / keep-mine /
                // diff banner instead of silently overwriting.
                if let Some(baseline) = self.pending_save_baseline.remove(&buffer_id) {
                    if let Some(stored) = self.editor.snapshot(buffer_id).and_then(|s| s.file) {
                        let _ = self.editor.set_file_association(
                            buffer_id,
                            Some(stored.with_content_hash(baseline)),
                        );
                    }
                }
                self.reconcile_after_save_conflict(buffer_id, content, file);
            }
            FileIoEvent::Reloaded {
                buffer_id,
                content,
                file,
            } => self.apply_reloaded_file(buffer_id, content, file),
            FileIoEvent::ExternalChanged {
                buffer_id,
                path: _,
                content,
                file,
            } => {
                // Clean buffer → silently reload; dirty buffer → raise the
                // reload / keep-mine / diff banner. One decision point for
                // every external-change trigger.
                self.reconcile_file_buffer(buffer_id, content, file);
            }
            FileIoEvent::Rechecked {
                buffer_id,
                content,
                file,
            } => {
                // One-shot disk recheck (session restore / explicit
                // refresh) — same reconciliation as a live external change.
                self.reconcile_file_buffer(buffer_id, content, file);
            }
            FileIoEvent::Deleted { buffer_id, path } => {
                // δ.3 — sticky banner. The rope stays in memory; the
                // file association is preserved so a subsequent
                // `file.save` recreates the path. The watcher already
                // dropped its `watched` entry.
                if self.tree.tabs.values().any(|t| t.buffer_id == buffer_id) {
                    self.file_banner = Some(FileBanner::new(format!(
                        "{} was deleted externally — buffer kept in memory. Save to recreate.",
                        path.display()
                    )));
                }
            }
            FileIoEvent::EncodingNotice { path, encoding } => {
                // δ.3 — sticky banner so the user knows the file
                // contained replacement characters before they
                // re-save and overwrite the original encoding.
                self.file_banner = Some(FileBanner::new(format!(
                    "{} appears to be {encoding} — opened with replacement characters. \
                     Re-save will write UTF-8 and discard the original encoding.",
                    path.display()
                )));
            }
            FileIoEvent::Failed {
                buffer_id,
                operation,
                path,
                reason,
            } => {
                // A failed write means the on-disk export is stale, so the
                // optimistic `mark_saved_clean` was wrong: roll the buffer's
                // content hash back to its pre-save value to re-flag dirty.
                if let Some(bid) = buffer_id {
                    if let Some(baseline) = self.pending_save_baseline.remove(&bid) {
                        if let Some(file) = self.editor.snapshot(bid).and_then(|snap| snap.file) {
                            let _ = self
                                .editor
                                .set_file_association(bid, Some(file.with_content_hash(baseline)));
                        }
                    }
                }
                let label = path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "file".into());
                self.file_banner = Some(FileBanner::new(format!(
                    "{operation} failed for {label}: {reason}"
                )));
            }
        }
    }

    pub(crate) fn mark_tab_file_associated(&mut self, buffer_id: BufferId, file: &FileAssociation) {
        for tab in self.tree.tabs.values_mut() {
            if tab.buffer_id == buffer_id {
                tab.file_associated = true;
                tab.label_override = file
                    .path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(str::to_string);
            }
        }
    }

    fn apply_reloaded_file(&mut self, buffer_id: BufferId, content: String, file: FileAssociation) {
        let Some(snap) = self.editor.snapshot(buffer_id) else {
            return;
        };
        let rope = snap.rope_snapshot().rope();
        let end = Position::from_byte_offset(rope, rope.len_bytes()).unwrap_or(Position::ZERO);
        let op = EditOp::replace(Range::new(Position::ZERO, end), content);
        if self.editor.apply_edit(buffer_id, op).is_ok() {
            self.cancel_display_prewarm_for_buffer(buffer_id);
            let _ = self
                .editor
                .set_file_association(buffer_id, Some(file.clone()));
            self.mark_tab_file_associated(buffer_id, &file);
            let now = self.now_ms();
            self.file_banner = Some(FileBanner::transient(
                format!("Reloaded {}", file.path.display()),
                now,
            ));
        }
    }

    pub(crate) fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}
