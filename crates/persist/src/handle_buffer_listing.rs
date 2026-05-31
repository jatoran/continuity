//! δ.4 — `PersistClient::list_buffer_records` wrapper.
//!
//! Sibling of [`crate::handle`] (kept separate so that file stays
//! under the 600-line cap). Thin wrapper around the
//! [`crate::PersistMessage::ListBufferRecords`] reply-channel.

use crossbeam_channel::bounded;

use crate::buffer_listing::{BufferListFilter, BufferRecord};
use crate::handle::PersistClient;
use crate::message::PersistMessage;
use crate::Error;

impl PersistClient {
    /// δ.4: synchronously enumerate every buffer row matching `filter`,
    /// sorted by `last_touched DESC` and joined with each row's latest
    /// snapshot for the derived title.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error`] the thread reports.
    pub fn list_buffer_records(
        &self,
        filter: BufferListFilter,
    ) -> Result<Vec<BufferRecord>, Error> {
        let (tx, rx) = bounded(1);
        self.sender()
            .send(PersistMessage::ListBufferRecords { filter, reply: tx })
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

        let rows = c.list_buffer_records(BufferListFilter::ActiveOnly).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, buf.id());
        assert_eq!(rows[0].title.as_deref(), Some("Title"));
    }

    #[test]
    fn empty_filter_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("h.db");
        let h = PersistHandle::spawn(&path).unwrap();
        let c = h.client();
        let rows = c.list_buffer_records(BufferListFilter::All).unwrap();
        assert!(rows.is_empty());
    }
}
