//! Phase-15 file command, drag/drop, and banner handling.
//!
//! UI methods in this module only show dialogs, route HWND drop messages,
//! and enqueue work to the file-I/O worker. Disk reads/writes happen on
//! `file_io`.

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
    pending: Option<PendingExternalChange>,
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

    fn external(buffer_id: BufferId, path: PathBuf) -> Self {
        Self {
            text: format!(
                "{} changed on disk - reload / keep mine / show diff",
                path.display()
            ),
            pending: Some(PendingExternalChange {
                buffer_id,
                path,
                disk_content: None,
            }),
            expires_at_ms: None,
        }
    }

    fn external_with_content(
        buffer_id: BufferId,
        path: PathBuf,
        disk_content: String,
        _disk_file: FileAssociation,
    ) -> Self {
        let mut banner = Self::external(buffer_id, path);
        if let Some(pending) = banner.pending.as_mut() {
            pending.disk_content = Some(disk_content);
        }
        banner
    }
}

#[derive(Clone, Debug)]
struct PendingExternalChange {
    buffer_id: BufferId,
    path: PathBuf,
    disk_content: Option<String>,
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
                ) {
                    reason = "external_invalidate";
                }
                self.handle_file_io_event(event);
                changed = true;
            }
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
    pub(crate) fn file_banner_overlay(&self, width: f32) -> Option<OverlayDraw> {
        let banner = self.file_banner.as_ref()?;
        Some(OverlayDraw {
            panel: PanelStyle {
                rect: DrawRect::new(12.0, 8.0, (width - 24.0).clamp(240.0, 760.0), 42.0),
                corner_radius: 6.0,
                bg: Rgba {
                    r: 0.12,
                    g: 0.13,
                    b: 0.15,
                    a: 0.96,
                },
                border: Rgba {
                    r: 0.42,
                    g: 0.47,
                    b: 0.55,
                    a: 1.0,
                },
                shadow: Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.35,
                },
                shadow_offset: 3.0,
            },
            input_focused: false,
            focus_field: Some(FocusField {
                rect: DrawRect::new(24.0, 17.0, (width - 48.0).clamp(200.0, 720.0), 20.0),
                text: banner.text.clone(),
                placeholder: None,
                caret_byte: banner.text.len(),
                selection_range: None,
                fg: Rgba {
                    r: 0.92,
                    g: 0.94,
                    b: 0.98,
                    a: 1.0,
                },
                selection_bg: Rgba::TRANSPARENT,
                placeholder_fg: Rgba::TRANSPARENT,
                caret_color: Rgba::TRANSPARENT,
                focus_ring: Rgba::TRANSPARENT,
            }),
            secondary_field: None,
            list_rows: Vec::new(),
            scrollbar: None,
            footer: Some(FooterText {
                rect: DrawRect::new(24.0, 32.0, 1.0, 1.0),
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
        self.enqueue_save(
            self.buffer_id,
            file.path.clone(),
            snap.rope_snapshot().rope().to_string(),
        )
    }

    pub(crate) fn file_save_as_impl(&mut self) -> Result<(), CommandError> {
        self.maybe_trim_trailing_whitespace_for_save();
        let snap = self
            .editor
            .snapshot(self.buffer_id)
            .ok_or(CommandError::UnsupportedContext("file_save_as"))?;
        let Some(path) = save_file_dialog(self.hwnd, snap.file.as_ref()) else {
            return Ok(());
        };
        self.enqueue_save(
            self.buffer_id,
            path,
            snap.rope_snapshot().rope().to_string(),
        )
    }

    /// Phase B14: run `SelectionEdit::TrimTrailingWhitespaceAll` as
    /// part of every save when the setting is enabled. The selection
    /// edit goes through the regular planning machinery so it lands
    /// as a single undo group and the snapshot taken afterwards sees
    /// the trimmed text.
    fn maybe_trim_trailing_whitespace_for_save(&mut self) {
        if !self.trim_trailing_whitespace_on_save {
            return;
        }
        let _ = self.editor.apply_selection_edit(
            self.buffer_id,
            continuity_core::SelectionEdit::TrimTrailingWhitespaceAll,
        );
    }

    fn enqueue_save(
        &mut self,
        buffer_id: BufferId,
        path: PathBuf,
        content: String,
    ) -> Result<(), CommandError> {
        let file_io = self
            .file_io
            .as_ref()
            .ok_or(CommandError::UnsupportedContext("file_save"))?;
        if file_io.save_buffer(buffer_id, path, content) {
            Ok(())
        } else {
            Err(CommandError::UnsupportedContext("file_save"))
        }
    }

    fn handle_file_io_event(&mut self, event: FileIoEvent) {
        match event {
            FileIoEvent::Opened {
                target_pane,
                content,
                file,
            } => self.adopt_opened_file(target_pane, content, file),
            FileIoEvent::DirectoryListed {
                root,
                relative,
                entries,
                truncated,
            } => self.handle_file_tree_directory_list(root, relative, entries, truncated),
            FileIoEvent::Saved { buffer_id, file } => {
                let _ = self
                    .editor
                    .set_file_association(buffer_id, Some(file.clone()));
                self.mark_tab_file_associated(buffer_id, &file);
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
            FileIoEvent::Reloaded {
                buffer_id,
                content,
                file,
            } => self.apply_reloaded_file(buffer_id, content, file),
            FileIoEvent::ExternalChanged {
                buffer_id,
                path,
                content,
                file,
            } => {
                if self.tree.tabs.values().any(|t| t.buffer_id == buffer_id) {
                    self.file_banner = Some(FileBanner::external_with_content(
                        buffer_id, path, content, file,
                    ));
                }
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
                operation,
                path,
                reason,
            } => {
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

    fn adopt_opened_file(
        &mut self,
        target_pane: Option<crate::pane_tree::PaneId>,
        content: String,
        file: FileAssociation,
    ) {
        let buffer_id = self.editor.open_file_buffer(content, file.clone());
        self.save_current_right_edge_chrome_state();
        if let Some(pane) = target_pane {
            self.switch_focus(pane);
        }
        let tab_id = self.tree.insert_fresh_buffer_tab(buffer_id, self.now_ms());
        if let Some(group) = self.tree.groups.get_mut(&self.tree.focused) {
            group.push_tab(tab_id, true);
        }
        self.apply_new_pane_state(buffer_id);
        self.mark_tab_file_associated(buffer_id, &file);
        self.refresh_focused_viewport();
        self.refresh_language();
        self.maybe_submit_decoration();
        // The new tab is itself the indicator that the file opened —
        // no banner needed. (Errors and external-change prompts still
        // surface a banner via the FileIoEvent::Failed / ExternalChanged
        // paths.)
        if let Some(file_io) = self.file_io.as_ref() {
            let _ = file_io.watch_file(buffer_id, file);
        }
        // P0.8.2 — file open lands a fresh buffer in the focused pane.
        // Any preceding `switch_focus(pane)` dispatched for the
        // *previous* buffer; this dispatch is for the just-opened one
        // so the next paint can hit a worker result instead of cold-
        // building a multi-thousand-line markdown file inline.
        let _ = self.try_dispatch_projection_worker_early("file_open", "focus_change");
        self.retarget_find_bar_to_focused_pane();
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

    pub(crate) fn file_reload_external_impl(&mut self) -> Result<(), CommandError> {
        let pending = self
            .file_banner
            .as_ref()
            .and_then(|banner| banner.pending.clone())
            .ok_or(CommandError::UnsupportedContext("file_reload_external"))?;
        let file_io = self
            .file_io
            .as_ref()
            .ok_or(CommandError::UnsupportedContext("file_reload_external"))?;
        if file_io.reload_buffer(pending.buffer_id, pending.path) {
            Ok(())
        } else {
            Err(CommandError::UnsupportedContext("file_reload_external"))
        }
    }

    pub(crate) fn file_keep_mine_impl(&mut self) -> Result<(), CommandError> {
        if self
            .file_banner
            .as_ref()
            .and_then(|b| b.pending.as_ref())
            .is_some()
        {
            self.file_banner = None;
            Ok(())
        } else {
            Err(CommandError::UnsupportedContext("file_keep_mine"))
        }
    }

    pub(crate) fn file_show_diff_impl(&mut self) -> Result<(), CommandError> {
        let Some(pending) = self
            .file_banner
            .as_ref()
            .and_then(|banner| banner.pending.clone())
        else {
            return Err(CommandError::UnsupportedContext("file_show_diff"));
        };
        let Some(disk_content) = pending.disk_content.as_deref() else {
            self.file_banner = Some(FileBanner::new(format!(
                "Diff unavailable for {}; reload / keep mine",
                pending.path.display()
            )));
            return Ok(());
        };
        let current_lines = self
            .editor
            .snapshot(pending.buffer_id)
            .map(|s| s.rope_snapshot().rope().len_lines())
            .unwrap_or(0);
        let disk_lines = disk_content.lines().count().max(1);
        self.file_banner = Some(FileBanner::new(format!(
            "Diff {}: editor {} lines, disk {} lines; reload / keep mine",
            pending.path.display(),
            current_lines,
            disk_lines
        )));
        Ok(())
    }

    pub(crate) fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}
