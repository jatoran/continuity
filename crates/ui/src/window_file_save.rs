//! Save-path helpers for [`crate::Window`].
//!
//! The UI thread owns the optimistic clean-state mutation. Actual disk
//! writes are still enqueued to the file-I/O worker thread.

use std::path::PathBuf;

use continuity_buffer::{BufferId, FileAssociation};
use continuity_command::Error as CommandError; // alias: collides with crate::Error

use crate::Window;

impl Window {
    /// Mark the buffer clean the instant a save is dispatched, instead of
    /// waiting for the asynchronous `FileIoEvent::Saved` round-trip.
    ///
    /// Dirty state is `rope_hash != FileAssociation.content_hash`
    /// (see [`crate::window_paint_builders::is_tab_dirty`]). The later
    /// `Saved` event re-sets the identical content hash plus the
    /// authoritative mtime + raw hash the file watcher needs, so this is a
    /// purely optimistic early clear; a subsequent keystroke re-flags dirty
    /// immediately.
    pub(crate) fn mark_saved_clean(
        &mut self,
        buffer_id: BufferId,
        base: FileAssociation,
        content: &str,
    ) {
        let content_hash = continuity_persist::fnv1a_64(content.as_bytes());
        // Remember the pre-save content hash so a failed write can roll the
        // optimistic clean back to dirty (see the `FileIoEvent::Failed`
        // handler). `or_insert` keeps the earliest baseline across rapid
        // re-saves — the value actually still on disk.
        self.pending_save_baseline
            .entry(buffer_id)
            .or_insert(base.content_hash);
        let _ = self
            .editor
            .set_file_association(buffer_id, Some(base.with_content_hash(content_hash)));
    }

    /// Run `SelectionEdit::TrimTrailingWhitespaceAll` as part of every save
    /// when the setting is enabled.
    pub(crate) fn maybe_trim_trailing_whitespace_for_save(&mut self) {
        if !self.trim_trailing_whitespace_on_save {
            return;
        }
        let _ = self.editor.apply_selection_edit(
            self.buffer_id,
            continuity_core::SelectionEdit::TrimTrailingWhitespaceAll,
        );
    }

    /// Queue `content` for an atomic file-system save on the file-I/O
    /// worker. `expected_hash` is the buffer's last-synced on-disk raw hash:
    /// when `Some`, the worker refuses the write (emitting `SaveConflict`)
    /// if the file changed externally since; `None` forces the write
    /// (save-as / "keep mine").
    pub(crate) fn enqueue_save(
        &mut self,
        buffer_id: BufferId,
        path: PathBuf,
        content: String,
        expected_hash: Option<u64>,
    ) -> Result<(), CommandError> {
        let file_io = self
            .file_io
            .as_ref()
            .ok_or(CommandError::UnsupportedContext("file_save"))?;
        if file_io.save_buffer(buffer_id, path, content, expected_hash) {
            Ok(())
        } else {
            Err(CommandError::UnsupportedContext("file_save"))
        }
    }
}
