//! External-change reconciliation for file-associated buffers.
//!
//! A file-associated buffer mirrors bytes on disk that other tools can
//! mutate. Three triggers expose divergence and every one funnels through
//! [`Window::reconcile_file_buffer`] so the decision is identical
//! everywhere:
//!
//! - the live `notify` watcher firing while the file is open
//!   ([`crate::window_file`] `ExternalChanged`),
//! - reopening an already-loaded path (registry reveal / spawn — see
//!   [`crate::window_file_open`] and the app registry),
//! - restoring a session whose file changed while continuity was closed
//!   (a one-shot recheck at launch — see [`crate::window_file_recheck`]).
//!
//! Decision table (given a buffer + freshly-read disk bytes):
//!
//! - disk fingerprint equals the buffer's stored association → no-op
//!   (this also absorbs our own writes, which already updated the
//!   association on `FileIoEvent::Saved`).
//! - disk changed, buffer **clean** (no unexported edits) → silently
//!   reload the new bytes, anchoring the caret line's screen position,
//!   and show a transient notice — unless `editor.auto_revert_unmodified`
//!   is off, in which case it banners like the dirty case.
//! - disk changed, buffer **dirty** (unexported edits) → raise the sticky
//!   reload / keep-mine / show-diff banner; never mutate the buffer.
//!
//! Thread ownership: UI thread of one window.

use continuity_buffer::{BufferId, FileAssociation};
use continuity_text::{EditOp, Position, Range};

use crate::window::Window;
use crate::window_file::FileBanner;

/// The outcome of comparing a buffer's stored file association against
/// freshly-read disk bytes. Pure decision, split out so it is testable
/// without a live [`Window`] (which would spawn a real Win32 surface).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReconcileAction {
    /// Disk fingerprint matches the stored association — our own write or
    /// no change. Do nothing.
    Noop,
    /// Disk changed and the buffer has no unexported edits — adopt the new
    /// bytes silently.
    SilentReload,
    /// Disk changed and either the buffer has unexported edits or
    /// auto-revert is disabled — surface the reload / keep-mine / diff
    /// banner.
    Banner,
}

/// Decide what reconciliation a fresh disk read implies. `dirty` is
/// whether the buffer carries unexported edits relative to `stored`;
/// `auto_revert` mirrors `editor.auto_revert_unmodified`.
pub(crate) fn classify_reconcile(
    stored: &FileAssociation,
    disk: &FileAssociation,
    dirty: bool,
    auto_revert: bool,
) -> ReconcileAction {
    if disk.hash == stored.hash && disk.content_hash == stored.content_hash {
        return ReconcileAction::Noop;
    }
    if dirty || !auto_revert {
        ReconcileAction::Banner
    } else {
        ReconcileAction::SilentReload
    }
}

impl Window {
    /// Reconcile a file-associated buffer against freshly-read disk
    /// bytes. See the module docs for the decision table. No-op when the
    /// buffer has no live snapshot or no file association.
    pub(crate) fn reconcile_file_buffer(
        &mut self,
        buffer_id: BufferId,
        content: String,
        file: FileAssociation,
    ) {
        self.reconcile_file_buffer_inner(buffer_id, content, file, false);
    }

    /// As [`Self::reconcile_file_buffer`], but flags the conflict as raised
    /// by a refused *save* so a resulting banner's "keep mine" force-writes
    /// the editor's version (see [`crate::window_file::PendingExternalChange`]).
    pub(crate) fn reconcile_after_save_conflict(
        &mut self,
        buffer_id: BufferId,
        content: String,
        file: FileAssociation,
    ) {
        self.reconcile_file_buffer_inner(buffer_id, content, file, true);
    }

    fn reconcile_file_buffer_inner(
        &mut self,
        buffer_id: BufferId,
        content: String,
        file: FileAssociation,
        from_save: bool,
    ) {
        let Some(snap) = self.editor.snapshot(buffer_id) else {
            return;
        };
        let Some(stored) = snap.file.clone() else {
            return;
        };
        let dirty = crate::window_paint_builders::is_buffer_dirty_against_file(self, buffer_id);
        match classify_reconcile(
            &stored,
            &file,
            dirty,
            self.view_options.auto_revert_unmodified,
        ) {
            ReconcileAction::Noop => {}
            ReconcileAction::Banner => {
                self.raise_external_change_banner(buffer_id, content, file, from_save)
            }
            ReconcileAction::SilentReload => {
                self.silent_reload_file_buffer(buffer_id, content, file)
            }
        }
    }

    /// Raise the sticky reload / keep-mine / show-diff banner carrying the
    /// disk bytes. Only fires when a tab for the buffer exists in this
    /// window — a banner with no on-screen buffer would be context-free;
    /// the reopen/reveal paths adopt a tab before reconciling.
    fn raise_external_change_banner(
        &mut self,
        buffer_id: BufferId,
        content: String,
        file: FileAssociation,
        from_save: bool,
    ) {
        if self.tree.tabs.values().any(|t| t.buffer_id == buffer_id) {
            let path = file.path.clone();
            self.file_banner = Some(FileBanner::external_with_content(
                buffer_id, path, content, file, from_save,
            ));
        }
    }

    /// Silently adopt the disk bytes for a clean buffer, anchoring the
    /// caret line's screen y when the buffer is the focused one, then show
    /// a transient confirmation.
    fn silent_reload_file_buffer(
        &mut self,
        buffer_id: BufferId,
        content: String,
        file: FileAssociation,
    ) {
        let focused = buffer_id == self.buffer_id;
        let applied = if focused {
            self.with_caret_line_anchored(|w| w.replace_buffer_content(buffer_id, content))
        } else {
            self.replace_buffer_content(buffer_id, content)
        };
        if !applied {
            return;
        }
        self.cancel_display_prewarm_for_buffer(buffer_id);
        let _ = self
            .editor
            .set_file_association(buffer_id, Some(file.clone()));
        self.mark_tab_file_associated(buffer_id, &file);
        let now = self.now_ms();
        self.file_banner = Some(FileBanner::transient(
            format!("Reloaded {} (changed on disk)", file.path.display()),
            now,
        ));
        // Re-arm the watch against the new fingerprint so a follow-up
        // external edit is still observed.
        if let Some(file_io) = self.file_io.as_ref() {
            let _ = file_io.watch_file(buffer_id, file);
        }
    }

    /// Replace the entire rope of `buffer_id` with `content` via one
    /// whole-buffer replace edit. Returns whether the edit applied.
    pub(crate) fn replace_buffer_content(&mut self, buffer_id: BufferId, content: String) -> bool {
        let Some(snap) = self.editor.snapshot(buffer_id) else {
            return false;
        };
        let rope = snap.rope_snapshot().rope();
        let end = Position::from_byte_offset(rope, rope.len_bytes()).unwrap_or(Position::ZERO);
        let op = EditOp::replace(Range::new(Position::ZERO, end), content);
        self.editor.apply_edit(buffer_id, op).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_reconcile, ReconcileAction};
    use continuity_buffer::FileAssociation;
    use std::path::PathBuf;

    fn assoc(hash: u64, content_hash: u64) -> FileAssociation {
        FileAssociation::new(PathBuf::from("note.md"), 1_700_000_000_000, hash)
            .with_content_hash(content_hash)
    }

    #[test]
    fn unchanged_disk_is_noop_even_when_dirty() {
        let stored = assoc(10, 20);
        let disk = assoc(10, 20);
        assert_eq!(
            classify_reconcile(&stored, &disk, true, true),
            ReconcileAction::Noop
        );
    }

    #[test]
    fn changed_clean_auto_reverts_silently() {
        let stored = assoc(10, 20);
        let disk = assoc(11, 21);
        assert_eq!(
            classify_reconcile(&stored, &disk, false, true),
            ReconcileAction::SilentReload
        );
    }

    #[test]
    fn changed_dirty_always_banners() {
        let stored = assoc(10, 20);
        let disk = assoc(11, 21);
        assert_eq!(
            classify_reconcile(&stored, &disk, true, true),
            ReconcileAction::Banner
        );
    }

    #[test]
    fn changed_clean_banners_when_auto_revert_off() {
        let stored = assoc(10, 20);
        let disk = assoc(11, 21);
        assert_eq!(
            classify_reconcile(&stored, &disk, false, false),
            ReconcileAction::Banner
        );
    }

    #[test]
    fn content_hash_drift_alone_counts_as_changed() {
        // Same raw-byte hash but different decoded-content hash (e.g. a
        // re-encode that round-trips to the same logical text differently)
        // still reconciles rather than no-ops.
        let stored = assoc(10, 20);
        let disk = assoc(10, 99);
        assert_eq!(
            classify_reconcile(&stored, &disk, false, true),
            ReconcileAction::SilentReload
        );
    }
}
