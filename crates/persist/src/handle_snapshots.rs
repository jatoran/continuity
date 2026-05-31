//! Snapshot operations on [`PersistClient`].
//!
//! Split out of [`crate::handle`] to keep that file under the 600-line
//! convention cap. The persistence-thread contract and channel
//! discipline are documented there; this module just hosts the
//! snapshot-shaped methods (async fire-and-forget, blocking save with
//! ack, and load-latest).

use std::sync::atomic::Ordering;

use continuity_buffer::{BufferId, RopeSnapshot};
use crossbeam_channel::bounded;

use crate::budget::snapshot_byte_cost;
use crate::handle::PersistClient;
use crate::message::PersistMessage;
use crate::store::SnapshotRow;
use crate::Error;

impl PersistClient {
    /// Persist a snapshot, fire-and-forget.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ThreadGone`] if the persistence thread has exited.
    pub fn save_snapshot_async(
        &self,
        buffer_id: BufferId,
        snapshot: RopeSnapshot,
    ) -> Result<(), Error> {
        let cost = snapshot_byte_cost(&snapshot);
        self.unflushed().fetch_add(cost, Ordering::AcqRel);
        match self.sender().send(PersistMessage::SaveSnapshot {
            buffer_id,
            snapshot,
            ack: None,
        }) {
            Ok(()) => Ok(()),
            Err(_) => {
                self.unflushed().fetch_sub(cost, Ordering::AcqRel);
                Err(Error::ThreadGone)
            }
        }
    }

    /// Persist a snapshot and wait for the row to be committed. Returns
    /// the assigned snapshot id.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error::Sqlite`] the thread reports, or
    /// [`Error::ThreadGone`] if the thread has exited.
    pub fn save_snapshot_blocking(
        &self,
        buffer_id: BufferId,
        snapshot: RopeSnapshot,
    ) -> Result<i64, Error> {
        let cost = snapshot_byte_cost(&snapshot);
        let (tx, rx) = bounded(1);
        self.unflushed().fetch_add(cost, Ordering::AcqRel);
        if self
            .sender()
            .send(PersistMessage::SaveSnapshot {
                buffer_id,
                snapshot,
                ack: Some(tx),
            })
            .is_err()
        {
            self.unflushed().fetch_sub(cost, Ordering::AcqRel);
            return Err(Error::ThreadGone);
        }
        rx.recv().map_err(|_| Error::ThreadGone)?
    }

    /// Synchronously load the most-recent valid snapshot for
    /// `buffer_id`. Returns `Ok(None)` on first run.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error`] the thread reports.
    pub fn load_latest_snapshot(&self, buffer_id: BufferId) -> Result<Option<SnapshotRow>, Error> {
        let (tx, rx) = bounded(1);
        self.sender()
            .send(PersistMessage::LoadLatestSnapshot {
                buffer_id,
                reply: tx,
            })
            .map_err(|_| Error::ThreadGone)?;
        rx.recv().map_err(|_| Error::ThreadGone)?
    }
}
