//! `PersistClient::list_buffer_history_timeline` wrapper.
//!
//! Sibling of [`crate::handle`] and [`crate::handle_buffer_listing`] —
//! kept separate so `handle.rs` stays under the 600-line cap. Thin
//! wrapper around the
//! [`crate::PersistMessage::ListBufferHistoryTimeline`] reply-channel.

use crossbeam_channel::bounded;

use crate::buffer_history::BufferHistoryLane;
use crate::buffer_listing::BufferListFilter;
use crate::handle::PersistClient;
use crate::message::PersistMessage;
use crate::Error;

impl PersistClient {
    /// Synchronously fetch the buffer-history swimlane data.
    ///
    /// Returns one [`BufferHistoryLane`] per buffer matching `filter`,
    /// in `last_touched DESC` order, each carrying that buffer's
    /// ascending snapshot timestamps plus line / char counts from the
    /// latest materialized content. The persist thread executes the
    /// underlying queries on its own connection and replies on a
    /// one-shot channel; this method blocks the calling (UI) thread
    /// until the reply lands. The call is infrequent (only fires when
    /// the history tab opens or the filter cycles) so the block is
    /// acceptable.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error`] the thread reports, or
    /// [`Error::ThreadGone`] when the persist thread has exited.
    pub fn list_buffer_history_timeline(
        &self,
        filter: BufferListFilter,
    ) -> Result<Vec<BufferHistoryLane>, Error> {
        let (tx, rx) = bounded(1);
        self.sender()
            .send(PersistMessage::ListBufferHistoryTimeline { filter, reply: tx })
            .map_err(|_| Error::ThreadGone)?;
        rx.recv().map_err(|_| Error::ThreadGone)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handle::PersistHandle;
    use continuity_buffer::Buffer;

    #[test]
    fn round_trip_through_handle() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("h.db");
        let h = PersistHandle::spawn(&path).unwrap();
        let c = h.client();

        let buf = Buffer::from_text("# Title\nbody\n");
        c.save_snapshot_blocking(buf.id(), buf.snapshot()).unwrap();

        let lanes = c
            .list_buffer_history_timeline(BufferListFilter::ActiveOnly)
            .unwrap();
        assert_eq!(lanes.len(), 1);
        assert_eq!(lanes[0].record.id, buf.id());
        assert_eq!(lanes[0].snapshot_times_ms.len(), 1);
        assert_eq!(lanes[0].line_count, 3);
        assert_eq!(lanes[0].char_count, 13);
    }

    #[test]
    fn empty_db_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("h.db");
        let h = PersistHandle::spawn(&path).unwrap();
        let c = h.client();
        let lanes = c
            .list_buffer_history_timeline(BufferListFilter::All)
            .unwrap();
        assert!(lanes.is_empty());
    }
}
