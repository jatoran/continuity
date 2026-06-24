//! Actions for the external-change banner: reload, keep-mine, show-diff.
//!
//! Split out of [`crate::window_file`] to keep that module under the
//! 600-line cap. These run when the user clicks one of the conflict
//! banner's buttons; the banner itself (and its [`crate::window_file::
//! FileBanner`] / `PendingExternalChange` payload) is defined there.
//!
//! Thread ownership: UI thread of one window.

use continuity_command::Error as CommandError; // alias: collides with crate::Error

use crate::window::Window;
use crate::window_file::FileBanner;

impl Window {
    /// Reload the file from disk, discarding the in-memory edits.
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

    /// Keep the in-memory edits, dismissing the conflict banner. When the
    /// conflict came from a refused *save* (`pending.from_save`), the user
    /// was actively trying to persist, so force-write their version to disk
    /// — otherwise the divergence persists and the next save just re-raises
    /// the same conflict. For a live-watcher conflict, "keep mine" only
    /// dismisses (don't disturb disk while an external tool may be mid-write).
    pub(crate) fn file_keep_mine_impl(&mut self) -> Result<(), CommandError> {
        let Some(pending) = self.file_banner.as_ref().and_then(|b| b.pending.clone()) else {
            return Err(CommandError::UnsupportedContext("file_keep_mine"));
        };
        self.file_banner = None;
        if !pending.from_save {
            return Ok(());
        }
        let Some(snap) = self.editor.snapshot(pending.buffer_id) else {
            return Ok(());
        };
        let Some(file) = snap.file.as_ref() else {
            return Ok(());
        };
        let path = file.path.clone();
        let base = file.clone();
        let content = snap.rope_snapshot().rope().to_string();
        self.mark_saved_clean(pending.buffer_id, base, &content);
        // Force the write (no conflict guard) — the user explicitly chose
        // their version over the external change.
        self.enqueue_save(pending.buffer_id, path, content, None)
    }

    /// Open a scratch tab showing a unified line diff between the editor
    /// buffer and the current file on disk.
    pub(crate) fn file_show_diff_impl(&mut self) -> Result<(), CommandError> {
        let Some(pending) = self
            .file_banner
            .as_ref()
            .and_then(|banner| banner.pending.clone())
        else {
            return Err(CommandError::UnsupportedContext("file_show_diff"));
        };
        let Some(disk_content) = pending.disk_content.clone() else {
            self.file_banner = Some(FileBanner::new(format!(
                "Diff unavailable for {}; reload / keep mine",
                pending.path.display()
            )));
            return Ok(());
        };
        let editor_content = self
            .editor
            .snapshot(pending.buffer_id)
            .map(|s| s.rope_snapshot().rope().to_string())
            .unwrap_or_default();
        let name = pending
            .path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("file")
            .to_string();
        let diff = crate::file_diff::unified_line_diff(&name, &editor_content, &disk_content);
        // Open the diff in a fresh scratch tab so it gets a real scrollable
        // surface instead of a one-line banner summary. The external-change
        // banner stays sticky — its reload / keep-mine actions still target
        // the original file buffer (`pending.buffer_id`), not this tab.
        let diff_buffer = self.editor.open_buffer(diff);
        self.adopt_buffer_as_new_tab(diff_buffer);
        self.tree
            .set_label_override_for_buffer(diff_buffer, &format!("Diff: {name}"));
        Ok(())
    }
}
