//! Snapshot timeline `PersistClient` methods.
//!
//! Sibling of [`crate::handle`]; every method is a thin wrapper around a
//! [`crate::PersistMessage`] variant. The request channel is the only
//! thread boundary.

use continuity_buffer::{BufferId, Revision};
use crossbeam_channel::bounded;

use crate::handle::PersistClient;
use crate::message::PersistMessage;
use crate::store::SnapshotSummaryRow;
use crate::Error;

impl PersistClient {
    /// Stamp `label` (or clear it via `None`) on the snapshot row at
    /// `revision`. Fire-and-forget; failures are logged.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ThreadGone`] when the persist thread has exited.
    pub fn set_snapshot_label(
        &self,
        buffer_id: BufferId,
        revision: Revision,
        label: Option<String>,
    ) -> Result<(), Error> {
        self.sender()
            .send(PersistMessage::SetSnapshotLabel {
                buffer_id,
                revision,
                label,
                reply: None,
            })
            .map_err(|_| Error::ThreadGone)
    }

    /// Synchronously enumerate every snapshot summary for `buffer_id`.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error`] the thread reports.
    pub fn list_snapshot_summaries(
        &self,
        buffer_id: BufferId,
    ) -> Result<Vec<SnapshotSummaryRow>, Error> {
        let (tx, rx) = bounded(1);
        self.sender()
            .send(PersistMessage::ListSnapshotSummaries {
                buffer_id,
                reply: tx,
            })
            .map_err(|_| Error::ThreadGone)?;
        rx.recv().map_err(|_| Error::ThreadGone)?
    }

    /// Synchronously materialize the rope content for `buffer_id` at
    /// `target_revision` by replaying persisted edits on top of the
    /// latest snapshot at-or-before `target_revision`.
    ///
    /// Returns `Ok(None)` when no snapshot exists at-or-before that
    /// revision. Read-only on the persistence side; used by the
    /// time-machine slider's preview render.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error`] the thread reports.
    pub fn load_content_at_revision(
        &self,
        buffer_id: BufferId,
        target_revision: Revision,
    ) -> Result<Option<String>, Error> {
        let (tx, rx) = bounded(1);
        self.sender()
            .send(PersistMessage::LoadContentAtRevision {
                buffer_id,
                target_revision,
                reply: tx,
            })
            .map_err(|_| Error::ThreadGone)?;
        rx.recv().map_err(|_| Error::ThreadGone)?
    }
}

#[cfg(test)]
mod tests {
    use continuity_buffer::Buffer;

    use super::*;
    use crate::handle::PersistHandle;

    #[test]
    fn load_content_at_revision_through_persist_client_matches_head() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("loadrev.db");
        let h = PersistHandle::spawn(&path).unwrap();
        let c = h.client();

        let buf = Buffer::from_text("hello");
        let snap = buf.snapshot();
        c.save_snapshot_blocking(buf.id(), snap.clone()).unwrap();

        let head = c
            .load_content_at_revision(buf.id(), snap.revision())
            .unwrap()
            .unwrap();
        assert_eq!(head, "hello");
    }

    #[test]
    fn load_content_at_unknown_buffer_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("loadrev.db");
        let h = PersistHandle::spawn(&path).unwrap();
        let c = h.client();
        let result = c
            .load_content_at_revision(BufferId::new(), Revision(0))
            .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn snapshot_summaries_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.db");
        let h = PersistHandle::spawn(&path).unwrap();
        let c = h.client();

        let buf = Buffer::from_text("hi");
        let snap = buf.snapshot();
        c.save_snapshot_blocking(buf.id(), snap.clone()).unwrap();
        c.set_snapshot_label(buf.id(), snap.revision(), Some("v1".into()))
            .unwrap();

        let summaries = c.list_snapshot_summaries(buf.id()).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].label.as_deref(), Some("v1"));
    }
}
