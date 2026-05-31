//! [`PersistHandle`] (lifetime owner) and [`PersistClient`] (clone-able
//! request side) for the persistence thread.
//!
//! **Thread ownership**: a [`Store`] (and therefore the underlying
//! [`rusqlite::Connection`]) lives only on the persistence thread. Everything
//! else holds a [`PersistClient`] — a thin wrapper around the request
//! channel's `Sender`.
//!
//! ## Channel size and overflow policy
//!
//! The request channel is bounded at [`CHANNEL_CAPACITY`] entries. All sends
//! use blocking `send`: the editor's promise is durability, so when the
//! pipeline is full we'd rather lag input than silently drop edit rows.
//!
//! Phase 17 adds a parallel **byte accountant** ([`PersistClient::unflushed_bytes`]):
//! every [`PersistMessage::AppendEdit`] and [`PersistMessage::SaveSnapshot`]
//! is sized by [`edit_row_byte_cost`] / [`snapshot_byte_cost`] and the gauge
//! is incremented on send, decremented after the persist thread handles
//! that message. When `unflushed_bytes() >= ` [`OVERLOAD_THRESHOLD_BYTES`]
//! the editor core thread is expected to coalesce adjacent
//! insert/delete/replace records per `(buffer, undo_group)` *before*
//! enqueueing further work — that bookkeeping is owned by core/undo, which
//! is the layer that knows the current undo group. The counter and
//! threshold here are the persistence-side contract.
//!
//! ## Lifetime / shutdown
//!
//! `PersistHandle::spawn` is the only constructor. It returns the unique
//! owner of the join handle. Drop the [`PersistHandle`] to shut the thread
//! down: a `Shutdown` message is enqueued *behind* any pending writes, so
//! the persist thread drains its backlog before acknowledging.
//!
//! [`PersistClient`]s are cheap clones of the request side. They can be
//! freely passed to the core thread, the backup scheduler, or anywhere else
//! that wants to enqueue persistence work; their `Drop` is a no-op.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use continuity_buffer::{BufferId, Revision};
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};

use crate::budget::{edit_row_byte_cost, OVERLOAD_THRESHOLD_BYTES};
use crate::events::PersistEvent;
use crate::message::PersistMessage;
use crate::persist_loop::persist_loop;
use crate::store::{EditRow, Store, UndoGroupRow};
use crate::Error;

/// Bound on the request channel. Sized so a several-second burst of typing
/// (well above realistic input rates) cannot fill the queue under normal
/// disk latency.
pub const CHANNEL_CAPACITY: usize = 8192;

/// Lifetime owner of the persistence thread.
///
/// Drop to enqueue a shutdown and join the thread. The shutdown is ordered
/// behind any pending writes, so a clean drop is also a flush.
pub struct PersistHandle {
    tx: Sender<PersistMessage>,
    unflushed: Arc<AtomicUsize>,
    /// δ.3 — receiver for write-failure / thread-stopped events. The
    /// app registry consumes this and fans events out to live windows.
    events_rx: Receiver<PersistEvent>,
    join: Option<JoinHandle<()>>,
}

impl PersistHandle {
    /// Spawn a persistence thread bound to the database at `path`. Opens (or
    /// creates) the database, runs migrations, and begins draining requests.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error::Sqlite`] from opening or migrating the
    /// database.
    pub fn spawn(path: &Path) -> Result<Self, Error> {
        let store = Store::open(path)?;
        // One-shot orphan sweep per process start. Removes any
        // `buffers` rows that have no edits, no snapshots, and no
        // file association — typically the residue of `tab.new`
        // sessions the user closed without typing. Idempotent; cheap
        // on small DBs and indexed on large ones (the inner
        // EXISTS clauses hit `idx_edits_buffer_revision` /
        // `idx_snapshots_buffer_revision`). Failures are logged
        // rather than fatal: a corrupt DB shouldn't block startup.
        match store.purge_orphan_buffers() {
            Ok(0) => {}
            Ok(n) => eprintln!("continuity-persist: purged {n} orphan buffer rows on startup"),
            Err(e) => eprintln!("continuity-persist: orphan sweep failed: {e}"),
        }
        let (tx, rx) = bounded::<PersistMessage>(CHANNEL_CAPACITY);
        let unflushed = Arc::new(AtomicUsize::new(0));
        let loop_unflushed = Arc::clone(&unflushed);
        // δ.3 — event channel. Unbounded so an inattentive consumer
        // never back-pressures the persist thread. The expected
        // steady-state event rate is zero (events fire only on errors
        // or shutdown).
        let (events_tx, events_rx) = unbounded::<PersistEvent>();
        let join = thread::Builder::new()
            .name("continuity-persist".into())
            .spawn(move || persist_loop(store, &rx, &loop_unflushed, events_tx))
            .expect("spawn continuity-persist thread");
        Ok(Self {
            tx,
            unflushed,
            events_rx,
            join: Some(join),
        })
    }

    /// A cheap, clone-able request handle.
    #[must_use]
    pub fn client(&self) -> PersistClient {
        PersistClient {
            tx: self.tx.clone(),
            unflushed: Arc::clone(&self.unflushed),
        }
    }

    /// δ.3 — borrow the receiver for [`PersistEvent`]s. The registry
    /// drains this in its main loop and fans events out to every live
    /// window via `WindowControl::PersistEvent`.
    #[must_use]
    pub fn events(&self) -> Receiver<PersistEvent> {
        self.events_rx.clone()
    }

    /// Send the shutdown message and wait for the thread to acknowledge,
    /// then join. Idempotent — repeated calls are no-ops once the join
    /// handle has been consumed.
    pub fn shutdown(&mut self) {
        let (tx, rx) = bounded(1);
        if self.tx.send(PersistMessage::Shutdown { reply: tx }).is_ok() {
            // Bounded wait: if the thread has died abnormally we don't want
            // to hang the app forever. Five seconds is generous for the
            // remaining-message drain.
            let _ = rx.recv_timeout(Duration::from_secs(5));
        }
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for PersistHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// A clone-able client for the persistence thread.
///
/// All convenience methods live here. Construct via [`PersistHandle::client`].
#[derive(Clone)]
pub struct PersistClient {
    tx: Sender<PersistMessage>,
    unflushed: Arc<AtomicUsize>,
}

impl PersistClient {
    /// Borrow the request sender (advanced use only — most callers want one
    /// of the typed methods below).
    #[must_use]
    pub fn sender(&self) -> &Sender<PersistMessage> {
        &self.tx
    }

    /// Approximate number of bytes that the persist thread has accepted
    /// from this client but not yet committed to disk. Updated whenever an
    /// `AppendEdit` or `SaveSnapshot` is sent and whenever the persist loop
    /// finishes one of those messages. The core thread reads this to decide
    /// when to start coalescing per spec §2.
    #[must_use]
    pub fn unflushed_bytes(&self) -> usize {
        self.unflushed.load(Ordering::Acquire)
    }

    /// Returns `true` once [`Self::unflushed_bytes`] crosses
    /// [`OVERLOAD_THRESHOLD_BYTES`]. The editor core thread is expected to
    /// coalesce subsequent edits per `(buffer, undo_group)` until this
    /// drops back to `false`.
    #[must_use]
    pub fn is_overloaded(&self) -> bool {
        self.unflushed_bytes() >= OVERLOAD_THRESHOLD_BYTES
    }

    /// Append an edit row. Blocks if the channel is full (see crate-level
    /// docs for the rationale).
    ///
    /// # Errors
    ///
    /// Returns [`Error::ThreadGone`] if the persistence thread has exited.
    pub fn append_edit(&self, row: EditRow) -> Result<(), Error> {
        self.append_edit_with_seq(row, None)
    }

    /// As [`Self::append_edit`] but tags the cross-thread message with
    /// a caller-provided edit sequence number for trace correlation.
    /// The persist worker binds the seq to its trace thread-local for
    /// the duration of the message handler.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ThreadGone`] if the persistence thread has exited.
    pub fn append_edit_with_seq(&self, row: EditRow, edit_seq: Option<u64>) -> Result<(), Error> {
        let cost = edit_row_byte_cost(&row);
        let _s = crate::trace::Scope::with_detail(
            "persist_client_append_edit_send",
            format!(
                "cost={cost} qlen={} unflushed={}",
                self.tx.len(),
                self.unflushed.load(Ordering::Relaxed),
            ),
        );
        self.unflushed.fetch_add(cost, Ordering::AcqRel);
        match self.tx.send(PersistMessage::AppendEdit { row, edit_seq }) {
            Ok(()) => Ok(()),
            Err(_) => {
                self.unflushed.fetch_sub(cost, Ordering::AcqRel);
                Err(Error::ThreadGone)
            }
        }
    }

    /// Borrow the unflushed-byte accountant. Used by sibling impls
    /// (see [`crate::handle_snapshots`]) that need the raw atomic.
    pub(crate) fn unflushed(&self) -> &Arc<AtomicUsize> {
        &self.unflushed
    }

    /// Insert (or refresh `last_touched` on) the buffers row. Fire-and-forget.
    ///
    /// # Errors
    ///
    /// See [`Self::append_edit`].
    pub fn upsert_buffer(
        &self,
        buffer_id: BufferId,
        created_at_ms: i64,
        last_touched_ms: i64,
    ) -> Result<(), Error> {
        self.tx
            .send(PersistMessage::UpsertBuffer {
                buffer_id,
                created_at_ms,
                last_touched_ms,
            })
            .map_err(|_| Error::ThreadGone)
    }

    /// Bump `last_touched` for a buffer. Fire-and-forget.
    ///
    /// # Errors
    ///
    /// See [`Self::append_edit`].
    pub fn touch_buffer(&self, buffer_id: BufferId, last_touched_ms: i64) -> Result<(), Error> {
        self.tx
            .send(PersistMessage::TouchBuffer {
                buffer_id,
                last_touched_ms,
            })
            .map_err(|_| Error::ThreadGone)
    }

    /// Synchronously fetch all edit rows after `after_revision`.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error`] the thread reports.
    pub fn edits_since(
        &self,
        buffer_id: BufferId,
        after_revision: Revision,
    ) -> Result<Vec<EditRow>, Error> {
        let (tx, rx) = bounded(1);
        self.tx
            .send(PersistMessage::EditsSince {
                buffer_id,
                after_revision,
                reply: tx,
            })
            .map_err(|_| Error::ThreadGone)?;
        rx.recv().map_err(|_| Error::ThreadGone)?
    }

    /// Synchronously fetch the most-recently-touched non-deleted buffer's id.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error`] the thread reports.
    pub fn most_recent_buffer(&self) -> Result<Option<BufferId>, Error> {
        let (tx, rx) = bounded(1);
        self.tx
            .send(PersistMessage::MostRecentBuffer { reply: tx })
            .map_err(|_| Error::ThreadGone)?;
        rx.recv().map_err(|_| Error::ThreadGone)?
    }

    /// Drop edit rows for `buffer_id` whose `revision <= at_or_before`.
    /// Fire-and-forget.
    ///
    /// # Errors
    ///
    /// See [`Self::append_edit`].
    pub fn prune_covered_edits(
        &self,
        buffer_id: BufferId,
        at_or_before: Revision,
    ) -> Result<(), Error> {
        self.tx
            .send(PersistMessage::PruneCoveredEdits {
                buffer_id,
                at_or_before,
            })
            .map_err(|_| Error::ThreadGone)
    }

    /// Move a buffer to the trash. Fire-and-forget.
    ///
    /// # Errors
    ///
    /// See [`Self::append_edit`].
    pub fn move_to_trash(
        &self,
        buffer_id: BufferId,
        now_ms: i64,
        retention_days: u32,
    ) -> Result<(), Error> {
        self.tx
            .send(PersistMessage::MoveToTrash {
                buffer_id,
                now_ms,
                retention_days,
            })
            .map_err(|_| Error::ThreadGone)
    }

    /// Synchronously hard-delete expired trash entries.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error`] the thread reports.
    pub fn purge_expired(&self, now_ms: i64) -> Result<usize, Error> {
        let (tx, rx) = bounded(1);
        self.tx
            .send(PersistMessage::PurgeExpired { now_ms, reply: tx })
            .map_err(|_| Error::ThreadGone)?;
        rx.recv().map_err(|_| Error::ThreadGone)?
    }

    /// Run the online backup synchronously.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error`] the thread reports.
    pub fn backup(&self, dest_path: PathBuf) -> Result<(), Error> {
        let (tx, rx) = bounded(1);
        self.tx
            .send(PersistMessage::Backup {
                dest_path,
                reply: Some(tx),
            })
            .map_err(|_| Error::ThreadGone)?;
        rx.recv().map_err(|_| Error::ThreadGone)?
    }

    /// Persist an undo-group row. Fire-and-forget.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ThreadGone`] if the persistence thread has exited.
    pub fn write_undo_group(&self, row: UndoGroupRow) -> Result<(), Error> {
        let _s = crate::trace::Scope::with_detail(
            "persist_client_write_undo_group_send",
            format!("qlen={}", self.tx.len()),
        );
        self.tx
            .send(PersistMessage::WriteUndoGroup { row })
            .map_err(|_| Error::ThreadGone)
    }

    /// Synchronously load every undo-group row for `buffer_id`, in `ts`
    /// order. Used at recovery time.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error`] the thread reports, or [`Error::ThreadGone`]
    /// if the thread has exited.
    pub fn load_undo_groups(&self, buffer_id: BufferId) -> Result<Vec<UndoGroupRow>, Error> {
        let (tx, rx) = bounded(1);
        self.tx
            .send(PersistMessage::LoadUndoGroups {
                buffer_id,
                reply: tx,
            })
            .map_err(|_| Error::ThreadGone)?;
        rx.recv().map_err(|_| Error::ThreadGone)?
    }

    /// Run the online backup, fire-and-forget. Errors are logged.
    ///
    /// # Errors
    ///
    /// See [`Self::append_edit`].
    pub fn backup_async(&self, dest_path: PathBuf) -> Result<(), Error> {
        self.tx
            .send(PersistMessage::Backup {
                dest_path,
                reply: None,
            })
            .map_err(|_| Error::ThreadGone)
    }

    /// Apply `PRAGMA synchronous = <value>` to the connection. Sent
    /// whenever `[persistence].mode` changes (Phase 12 live-reload).
    /// Fire-and-forget; failures are logged on the persist thread.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ThreadGone`] if the persistence thread has exited.
    pub fn set_synchronous(&self, value: &str) -> Result<(), Error> {
        self.tx
            .send(PersistMessage::SetSynchronous {
                value: value.to_string(),
            })
            .map_err(|_| Error::ThreadGone)
    }
}

#[cfg(test)]
mod tests {
    use continuity_buffer::Buffer;
    use continuity_text::{EditOp, Position};

    use super::*;

    #[test]
    fn round_trip_through_handle() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("h.db");
        let mut h = PersistHandle::spawn(&path).unwrap();
        let c = h.client();

        let mut buf = Buffer::from_text("hi");
        let id = c.save_snapshot_blocking(buf.id(), buf.snapshot()).unwrap();
        assert!(id > 0);

        buf.apply(&EditOp::insert(Position::new(0, 2), "!"))
            .unwrap();
        c.save_snapshot_blocking(buf.id(), buf.snapshot()).unwrap();

        let snap = c.load_latest_snapshot(buf.id()).unwrap().unwrap();
        assert_eq!(snap.content, "hi!");

        h.shutdown();
    }

    #[test]
    fn most_recent_buffer_through_client() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("h.db");
        let h = PersistHandle::spawn(&path).unwrap();
        let c = h.client();

        assert!(c.most_recent_buffer().unwrap().is_none());

        let id = BufferId::new();
        c.upsert_buffer(id, 100, 100).unwrap();
        // The synchronous most_recent_buffer query forces the upsert to
        // process before we observe it.
        let latest = c.most_recent_buffer().unwrap().unwrap();
        assert_eq!(latest, id);
    }

    #[test]
    fn drop_shuts_down_thread() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("h.db");
        {
            let _h = PersistHandle::spawn(&path).unwrap();
        }
        // If Drop didn't block on join, opening again could race. The fact
        // that we can re-open without error means the connection was
        // released cleanly.
        let _h2 = PersistHandle::spawn(&path).unwrap();
    }

    /// δ.3 — clean shutdown emits a `ThreadStopped` event before the
    /// Sender drops, so the registry can distinguish a planned exit
    /// from a thread panic.
    #[test]
    fn clean_shutdown_emits_thread_stopped_event() {
        use crate::events::PersistEvent;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("e.db");
        let mut h = PersistHandle::spawn(&path).unwrap();
        let events = h.events();
        h.shutdown();
        // Drain the channel and assert ThreadStopped appears.
        let mut saw_stopped = false;
        while let Ok(event) = events.recv_timeout(std::time::Duration::from_millis(500)) {
            if matches!(event, PersistEvent::ThreadStopped) {
                saw_stopped = true;
                break;
            }
        }
        assert!(saw_stopped, "expected PersistEvent::ThreadStopped");
    }

    #[test]
    fn unflushed_bytes_drains_to_zero() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("u.db");
        let h = PersistHandle::spawn(&path).unwrap();
        let c = h.client();
        assert_eq!(c.unflushed_bytes(), 0);

        let buf = Buffer::from_text("x".repeat(128).as_str());
        // Blocking snapshot: by the time it returns the persist loop has
        // accounted for and drained the cost.
        c.save_snapshot_blocking(buf.id(), buf.snapshot()).unwrap();
        // The next synchronous round-trip forces the persist loop to have
        // observed our decrement (the loop runs the requests in order).
        let _ = c.most_recent_buffer().unwrap();
        assert_eq!(c.unflushed_bytes(), 0);
    }

    #[test]
    fn client_outlives_clones() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("h.db");
        let h = PersistHandle::spawn(&path).unwrap();
        let c1 = h.client();
        let c2 = c1.clone();
        // Both clones can drive the persist thread.
        let id = BufferId::new();
        c1.upsert_buffer(id, 1, 1).unwrap();
        let latest = c2.most_recent_buffer().unwrap().unwrap();
        assert_eq!(latest, id);
    }
}
