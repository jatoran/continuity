//! One-shot disk recheck of restored file-associated buffers.
//!
//! Session restore rebuilds file buffers from the database snapshot — it
//! never reads disk. If the file changed while continuity was closed, the
//! restored tab would otherwise show stale bytes with no signal (and a
//! later save would clobber the external edit). At window construction we
//! kick a single recheck per restored file-associated buffer; the worker
//! reads the current bytes, (re)arms the external-change watch, and routes
//! a [`crate::file_io::FileIoEvent::Rechecked`] back to this window, which
//! reconciles it through [`Window::reconcile_file_buffer`].
//!
//! Thread ownership: UI thread of one window.

use std::collections::HashSet;

use crate::window::Window;

impl Window {
    /// Recheck every file-associated buffer currently shown in this
    /// window against its on-disk bytes. Safe to call repeatedly — the
    /// reconcile step no-ops when nothing changed, and a missing file is
    /// ignored (the rope stays canonical). No-op without a file-I/O
    /// worker (test windows).
    pub(crate) fn recheck_restored_file_buffers(&mut self) {
        let Some(file_io) = self.file_io.clone() else {
            return;
        };
        let reply = self.file_open_tx.clone();
        let mut seen = HashSet::new();
        let targets: Vec<_> = self
            .tree
            .tabs
            .values()
            .map(|t| t.buffer_id)
            .filter(|id| seen.insert(*id))
            .filter_map(|id| {
                self.editor
                    .snapshot(id)
                    .and_then(|snap| snap.file)
                    .map(|file| (id, file.path))
            })
            .collect();
        for (buffer_id, path) in targets {
            let _ = file_io.recheck_file(buffer_id, path, reply.clone());
        }
    }
}
