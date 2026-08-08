//! `Store`: the SQLite connection wrapper that owns persistence I/O.
//!
//! This module is the parent of the responsibility-scoped sibling
//! files below. The parent holds the row structs every consumer
//! shares, the `Store` wrapper itself, its constructor + connection
//! accessors, and the small helpers each sibling reuses
//! (`unix_ms_now`, `uuid_from_blob`). Every other `impl Store {…}`
//! block lives in one of the sibling files, grouped by table family:
//!
//! - [`snapshots`] — `buffer_snapshots` writes/reads + corruption fallback.
//! - [`edits`] — `buffer_edits` append/replay/prune.
//! - [`buffers`] — `buffers` upsert/touch and most-recent lookup.
//! - [`trash`] — trash insertion + cascade purge.
//! - [`undo_groups`] — `undo_groups` writes/reads.
//! - [`backup`] — SQL-level online backup driver.
//!
//! **Thread ownership**: a single persistence thread. Other threads
//! communicate via `crossbeam_channel`s consumed inside this thread.

use std::time::{SystemTime, UNIX_EPOCH};

use continuity_buffer::{BufferId, Revision, UndoGroupId};
use rusqlite::Connection;
use uuid::Uuid;

use std::path::Path;

use crate::{schema, Error};

mod backup;
mod buffers;
mod edits;
mod known_vaults;
mod snapshots;
mod trash;
mod undo_groups;

/// One day in milliseconds. Used by [`Store::move_to_trash`] to compute
/// expiry.
pub(crate) const ONE_DAY_MS: i64 = 86_400_000;

/// One snapshot row.
#[derive(Debug, Clone)]
pub struct SnapshotRow {
    /// Auto-increment id assigned by SQLite (None when not yet inserted).
    pub id: Option<i64>,
    /// The buffer this snapshot belongs to.
    pub buffer_id: BufferId,
    /// Revision the snapshot was taken at.
    pub revision: Revision,
    /// Decompressed content.
    pub content: String,
    /// Number of bytes in `content`.
    pub byte_len: u64,
    /// Number of lines in `content`.
    pub line_count: u32,
    /// FNV-1a checksum of the uncompressed content.
    pub checksum: u64,
    /// Phase I1: optional user-supplied label (e.g. `"pre-refactor"`).
    /// Set via [`Store::set_snapshot_label`]. `None` for unlabelled
    /// snapshots and every historical row written before the v4
    /// migration.
    pub label: Option<String>,
    /// Phase I1 (parallel wire-up bridge): unix-ms timestamp the row
    /// was committed. The timeline path needs this to label slider
    /// ticks; the writer path defaults to `0` until I1's writer
    /// projection lands.
    pub created_at_ms: i64,
}

/// One row of the timeline summary returned by
/// [`Store::list_snapshot_summaries`] (Phase I1). Lightweight enough
/// to populate the time-machine slider's tick set without
/// decompressing snapshot bodies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotSummaryRow {
    /// Snapshot revision (corresponds to the buffer revision the
    /// snapshot was taken at).
    pub revision: Revision,
    /// Unix milliseconds the snapshot was committed.
    pub created_at_ms: i64,
    /// Optional user-supplied label.
    pub label: Option<String>,
}

/// One edit-log row.
#[derive(Debug, Clone)]
pub struct EditRow {
    /// The buffer this edit belongs to.
    pub buffer_id: BufferId,
    /// Per-buffer monotonic sequence.
    pub seq: u64,
    /// Buffer revision after this edit.
    pub revision: Revision,
    /// Unix epoch milliseconds.
    pub ts_ms: i64,
    /// `"insert" | "delete" | "replace"`.
    pub op_kind: String,
    /// Position bounds (start_line, start_byte, end_line, end_byte). `None` for
    /// fields not relevant to the op kind.
    pub range_start_line: Option<u32>,
    /// See [`Self::range_start_line`].
    pub range_start_byte: Option<u32>,
    /// See [`Self::range_start_line`].
    pub range_end_line: Option<u32>,
    /// See [`Self::range_start_line`].
    pub range_end_byte: Option<u32>,
    /// Inserted text (insert / replace).
    pub inserted_text: Option<String>,
    /// Removed text (delete / replace).
    pub removed_text: Option<String>,
    /// JSON array of selections at the moment before the edit.
    pub selections_before_json: Option<String>,
    /// JSON array of selections at the moment after the edit.
    pub selections_after_json: Option<String>,
    /// Undo-group id this edit belongs to.
    pub undo_group_id: Option<Uuid>,
    /// FNV-1a checksum of the rope content after applying this edit.
    pub checksum_after: u64,
}

/// One persisted [`UndoGroup`](continuity_buffer::UndoGroup) row.
#[derive(Debug, Clone)]
pub struct UndoGroupRow {
    /// Undo-group id (UUIDv7).
    pub id: UndoGroupId,
    /// The buffer this group belongs to.
    pub buffer_id: BufferId,
    /// Command that produced the group.
    pub command_name: String,
    /// Wall-clock millis the group was created.
    pub ts_ms: i64,
    /// Parent group id, `None` for the root.
    pub parent_group_id: Option<UndoGroupId>,
}

/// SQLite-backed persistence store.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open or create a database at `path`.
    pub fn open(path: &Path) -> Result<Self, Error> {
        let conn = Connection::open(path)?;
        schema::migrate(&conn)?;
        Ok(Self { conn })
    }

    /// Open an in-memory database (for tests).
    pub fn open_in_memory() -> Result<Self, Error> {
        let conn = Connection::open_in_memory()?;
        schema::migrate(&conn)?;
        Ok(Self { conn })
    }

    /// Borrow the inner connection (for advanced use).
    #[must_use]
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Apply `PRAGMA synchronous = <value>`. Accepts only `"NORMAL"`,
    /// `"FULL"`, or `"OFF"` (the three values the spec's persistence-mode
    /// profile maps onto).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Sqlite`] when the PRAGMA write fails. Returns
    /// [`Error::InvalidArgument`] for an unrecognized value.
    pub fn set_synchronous(&self, value: &str) -> Result<(), Error> {
        match value {
            "NORMAL" | "FULL" | "OFF" => {
                self.conn.pragma_update(None, "synchronous", value)?;
                Ok(())
            }
            other => Err(Error::InvalidArgument(format!(
                "synchronous PRAGMA value `{other}` not in {{NORMAL, FULL, OFF}}"
            ))),
        }
    }
}

/// Parse a 16-byte SQLite blob into a UUID. Returns `None` for
/// wrong-length input so the caller can map it to a sensible domain
/// error.
pub(crate) fn uuid_from_blob(bytes: &[u8]) -> Option<Uuid> {
    <[u8; 16]>::try_from(bytes).ok().map(Uuid::from_bytes)
}

/// Current unix-epoch milliseconds, clamped to `i64::MAX` if the wall
/// clock somehow returns a value past the 2262 horizon.
pub(crate) fn unix_ms_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_runs_clean() {
        let _store = Store::open_in_memory().unwrap();
    }
}
