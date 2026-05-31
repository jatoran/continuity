//! Messages exchanged with the persistence thread.
//!
//! Mirrors the request/reply pattern of `continuity-core::EditorMessage`:
//! fire-and-forget for hot writes (edits, snapshots) and reply-channel-bearing
//! for queries.

use std::path::PathBuf;

use continuity_buffer::{BufferId, FileAssociation, Revision, RopeSnapshot, WindowId};
use crossbeam_channel::Sender;

use crate::buffer_history::BufferHistoryLane;
use crate::buffer_listing::{BufferListFilter, BufferRecord};
use crate::closed_history::{ClosedHistoryEntry, ClosedHistoryKind};
use crate::store::{
    EditRow, MetricsDailyDelta, MetricsDailyRow, SnapshotRow, SnapshotSummaryRow, TopBufferRow,
    UndoGroupRow,
};
use crate::window_state::WindowRow;
use crate::Error;

/// A request sent to the persistence thread.
///
/// Reply-bearing variants carry a [`Sender`] the thread writes the result
/// into; senders that disconnect before the result lands are silently
/// dropped (the caller has gone away).
pub enum PersistMessage {
    /// Append one [`EditRow`] to the edit log. Fire-and-forget.
    AppendEdit {
        /// The fully-encoded edit row.
        row: EditRow,
        /// Optional UI-thread edit sequence for cross-thread trace
        /// correlation. The persist worker binds this to its
        /// thread-local edit_seq for the duration of the message
        /// handler so every emitted scope carries the same
        /// `edit_seq=N` field as the UI/core scopes that produced the
        /// row.
        edit_seq: Option<u64>,
    },
    /// Persist a [`RopeSnapshot`] for `buffer_id`. If `ack` is `Some`, the
    /// thread sends the assigned snapshot id (or an error) when the row has
    /// been committed; otherwise the snapshot is fire-and-forget.
    SaveSnapshot {
        /// Buffer being snapshotted.
        buffer_id: BufferId,
        /// The snapshot to persist.
        snapshot: RopeSnapshot,
        /// Optional ack channel.
        ack: Option<Sender<Result<i64, Error>>>,
    },
    /// Insert (or no-op-update) the `buffers` row. Fire-and-forget.
    UpsertBuffer {
        /// The buffer's id.
        buffer_id: BufferId,
        /// Unix milliseconds of creation.
        created_at_ms: i64,
        /// Unix milliseconds of last activity.
        last_touched_ms: i64,
    },
    /// Bump `last_touched` for an existing buffer. Fire-and-forget.
    TouchBuffer {
        /// Target buffer.
        buffer_id: BufferId,
        /// New `last_touched` (Unix ms).
        last_touched_ms: i64,
    },
    /// Load the most recent valid snapshot for a buffer (with corruption
    /// fallback). Replies with `Ok(None)` on first run.
    LoadLatestSnapshot {
        /// Target buffer.
        buffer_id: BufferId,
        /// Reply channel.
        reply: Sender<Result<Option<SnapshotRow>, Error>>,
    },
    /// Return all edit rows for a buffer with revision strictly greater than
    /// `after_revision`, in `seq` order.
    EditsSince {
        /// Target buffer.
        buffer_id: BufferId,
        /// Lower bound (exclusive) on revision.
        after_revision: Revision,
        /// Reply channel.
        reply: Sender<Result<Vec<EditRow>, Error>>,
    },
    /// Return the most-recently-touched non-deleted buffer's id.
    MostRecentBuffer {
        /// Reply channel.
        reply: Sender<Result<Option<BufferId>, Error>>,
    },
    /// Drop edit rows ≤ a given revision (after a snapshot covers them).
    PruneCoveredEdits {
        /// Target buffer.
        buffer_id: BufferId,
        /// Drop edits with `revision <= at_or_before`.
        at_or_before: Revision,
    },
    /// Move a buffer to the trash. Fire-and-forget.
    MoveToTrash {
        /// Target buffer.
        buffer_id: BufferId,
        /// Now (Unix ms) — used as `deleted_at`.
        now_ms: i64,
        /// Retention window in days; used to compute `expires_at`.
        retention_days: u32,
    },
    /// Hard-delete trash rows whose `expires_at <= now_ms`. Replies with the
    /// number of rows purged.
    PurgeExpired {
        /// Now (Unix ms).
        now_ms: i64,
        /// Reply channel.
        reply: Sender<Result<usize, Error>>,
    },
    /// Run the SQLite online backup to `dest_path`. If `reply` is `None`,
    /// the backup is fire-and-forget (errors are logged by the persist
    /// thread).
    Backup {
        /// Destination database file.
        dest_path: PathBuf,
        /// Optional reply channel.
        reply: Option<Sender<Result<(), Error>>>,
    },
    /// Apply `PRAGMA synchronous = <value>` to the connection. Sent by
    /// `app` whenever `[persistence].mode` changes (live-reload Phase 12).
    /// Fire-and-forget; failures are logged.
    SetSynchronous {
        /// One of `"NORMAL" | "FULL" | "OFF"`. The persist thread
        /// validates and ignores any other value (with an `eprintln!`).
        value: String,
    },
    /// Insert an undo-group row. Fire-and-forget.
    WriteUndoGroup {
        /// The fully-encoded group row.
        row: UndoGroupRow,
    },
    /// Read every undo-group row for a buffer.
    LoadUndoGroups {
        /// Target buffer.
        buffer_id: BufferId,
        /// Reply channel.
        reply: Sender<Result<Vec<UndoGroupRow>, Error>>,
    },
    /// Upsert a row into the `windows` table (Phase 14). If `reply` is
    /// `Some`, the persist thread acks once committed; otherwise the upsert
    /// is fire-and-forget.
    SaveWindow {
        /// The full row payload.
        row: WindowRow,
        /// Optional ack channel.
        reply: Option<Sender<Result<(), Error>>>,
    },
    /// Soft-delete a window (stamps `deleted_at`).
    DeleteWindow {
        /// Target window.
        id: WindowId,
        /// Now (Unix ms) — used as `deleted_at`.
        now_ms: i64,
        /// Reply channel; `Ok(true)` if a row was updated.
        reply: Sender<Result<bool, Error>>,
    },
    /// Read every non-deleted `windows` row, most-recently-seen first.
    LoadActiveWindows {
        /// Reply channel.
        reply: Sender<Result<Vec<WindowRow>, Error>>,
    },
    /// Update file metadata on the `buffers` row.
    SetBufferFile {
        /// Target buffer.
        buffer_id: BufferId,
        /// New association, or `None` to clear it.
        file: Option<FileAssociation>,
        /// Optional ack channel.
        reply: Option<Sender<Result<(), Error>>>,
    },
    /// Load file metadata from the `buffers` row.
    LoadBufferFile {
        /// Target buffer.
        buffer_id: BufferId,
        /// Reply channel.
        reply: Sender<Result<Option<FileAssociation>, Error>>,
    },
    /// Load every active buffer id.
    LoadActiveBufferIds {
        /// Reply channel.
        reply: Sender<Result<Vec<BufferId>, Error>>,
    },
    /// Load the next edit sequence number for a buffer.
    NextSeq {
        /// Target buffer.
        buffer_id: BufferId,
        /// Reply channel.
        reply: Sender<Result<u64, Error>>,
    },
    /// Phase I1: stamp `label` onto the snapshot at the supplied
    /// revision. Pass `None` to clear the label.
    SetSnapshotLabel {
        /// Target buffer.
        buffer_id: BufferId,
        /// Snapshot revision to label.
        revision: Revision,
        /// New label, or `None` to clear.
        label: Option<String>,
        /// Optional ack channel; `Ok(rows_updated)`.
        reply: Option<Sender<Result<usize, Error>>>,
    },
    /// Phase I1: enumerate every snapshot for `buffer_id` as a
    /// timeline-slider summary (no decompression).
    ListSnapshotSummaries {
        /// Target buffer.
        buffer_id: BufferId,
        /// Reply channel.
        reply: Sender<Result<Vec<SnapshotSummaryRow>, Error>>,
    },
    /// Phase I1: materialize the rope content for `buffer_id` at the
    /// supplied revision by replaying persisted edits on top of the
    /// latest snapshot at-or-before that revision. Reply is `Ok(None)`
    /// when no snapshot exists at-or-before the target.
    LoadContentAtRevision {
        /// Target buffer.
        buffer_id: BufferId,
        /// Revision to reconstruct.
        target_revision: Revision,
        /// Reply channel — carries the materialized content.
        reply: Sender<Result<Option<String>, Error>>,
    },
    /// Phase I2: merge a [`MetricsDailyDelta`] into today's row.
    /// Fire-and-forget (the editor never blocks on metrics).
    RecordMetricsDelta {
        /// The delta to apply.
        delta: MetricsDailyDelta,
    },
    /// Phase I2: load every metric row inside the inclusive ISO-date
    /// window `[start, end]`, ordered ascending by day.
    LoadMetricsRange {
        /// Inclusive lower bound (`YYYY-MM-DD`).
        start_day_iso: String,
        /// Inclusive upper bound (`YYYY-MM-DD`).
        end_day_iso: String,
        /// Reply channel.
        reply: Sender<Result<Vec<MetricsDailyRow>, Error>>,
    },
    /// Phase I2: drop every row from `metrics_daily`. Replies with the
    /// number of rows removed.
    PurgeMetrics {
        /// Reply channel.
        reply: Sender<Result<usize, Error>>,
    },
    /// Phase I2: rank buffers by edit count inside `[start_ms, end_ms)`
    /// and return the top `limit`.
    LoadTopBuffersByEdits {
        /// Inclusive lower bound (unix ms).
        start_ms: i64,
        /// Exclusive upper bound (unix ms).
        end_ms: i64,
        /// Max rows to return.
        limit: usize,
        /// Reply channel.
        reply: Sender<Result<Vec<TopBufferRow>, Error>>,
    },
    /// δ.4: enumerate `buffers` rows for the previous-buffer browser,
    /// joined with each row's latest snapshot so the persist thread
    /// can decode a derived title inline.
    ListBufferRecords {
        /// Subset filter — active / all / trashed-only.
        filter: BufferListFilter,
        /// Reply channel.
        reply: Sender<Result<Vec<BufferRecord>, Error>>,
    },
    /// Enumerate buffer-history swimlane data for the buffer-history
    /// tab. One [`BufferHistoryLane`] per matching buffer, each carrying
    /// the ascending snapshot timestamps plus current line / char
    /// counts for its row.
    ListBufferHistoryTimeline {
        /// Subset filter — active / all / trashed-only.
        filter: BufferListFilter,
        /// Reply channel.
        reply: Sender<Result<Vec<BufferHistoryLane>, Error>>,
    },
    /// Push one entry onto the schema-v5 closed-history stack and evict
    /// the oldest entry beyond [`crate::closed_history::STACK_CAP`].
    PushClosedHistory {
        /// What kind of unit was closed (today: always `Window`).
        kind: ClosedHistoryKind,
        /// Closed window id when known.
        window_id: Option<WindowId>,
        /// Payload JSON — for window entries this is the pane-tree blob.
        payload_json: String,
        /// Wall-clock millis of the close event.
        closed_at_ms: i64,
        /// Reply channel.
        reply: Sender<Result<(), Error>>,
    },
    /// Pop the newest entry from the closed-history stack.
    PopClosedHistory {
        /// Reply channel.
        reply: Sender<Result<Option<ClosedHistoryEntry>, Error>>,
    },
    /// Peek the newest entry from the closed-history stack.
    PeekClosedHistory {
        /// Reply channel.
        reply: Sender<Result<Option<ClosedHistoryEntry>, Error>>,
    },
    /// Stop the thread. The thread acks once any pending writes have been
    /// processed.
    Shutdown {
        /// Reply channel for the ack.
        reply: Sender<()>,
    },
}
