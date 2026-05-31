//! `buffers` table reads/writes that are not snapshot- or trash-shaped:
//! upsert on adopt, last-touched bumps, and the startup-time
//! most-recent-buffer lookup.
//!
//! Thread ownership: the persistence thread (same as the parent
//! `Store`).

use continuity_buffer::BufferId;
use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

use super::Store;
use crate::Error;

impl Store {
    /// The id of the most-recently-touched non-deleted buffer, or `None` if
    /// no buffers exist.
    ///
    /// Used at startup to decide which buffer to restore.
    pub fn most_recent_buffer(&self) -> Result<Option<BufferId>, Error> {
        let row: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT id FROM buffers
                 WHERE deleted_at IS NULL
                 ORDER BY last_touched DESC
                 LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        Ok(row.and_then(|bytes| {
            <[u8; 16]>::try_from(bytes.as_slice())
                .ok()
                .map(|b| BufferId::from_uuid(Uuid::from_bytes(b)))
        }))
    }

    /// Insert (or no-op-update) the `buffers` row for `buffer_id`. Used by
    /// the persistence thread when adopting a freshly-opened buffer.
    pub fn upsert_buffer(
        &self,
        buffer_id: BufferId,
        created_at_ms: i64,
        last_touched_ms: i64,
    ) -> Result<(), Error> {
        self.conn.execute(
            "INSERT INTO buffers (id, created_at, last_touched, current_revision)
             VALUES (?1, ?2, ?3, 0)
             ON CONFLICT(id) DO UPDATE SET last_touched = excluded.last_touched",
            params![
                buffer_id.as_uuid().as_bytes(),
                created_at_ms,
                last_touched_ms
            ],
        )?;
        Ok(())
    }

    /// Bump `last_touched` for a buffer. Cheap; called whenever a snapshot
    /// or activity event indicates the buffer is "current".
    pub fn touch_buffer(&self, buffer_id: BufferId, last_touched_ms: i64) -> Result<(), Error> {
        self.conn.execute(
            "UPDATE buffers SET last_touched = ?2 WHERE id = ?1",
            params![buffer_id.as_uuid().as_bytes(), last_touched_ms],
        )?;
        Ok(())
    }

    /// Hard-delete every `buffers` row that has no edits, no
    /// snapshots, and no file association. Returns the number of
    /// rows removed. Idempotent — safe to call on every startup.
    ///
    /// These rows are the residue of `tab.new` sessions where the
    /// user opened a fresh buffer and closed the tab without typing.
    /// Without this sweep they accumulate forever and show up as
    /// "Untitled · just now · 0 edits" clutter in the
    /// previous-buffer browser overlay and the buffer-history tab.
    ///
    /// Trashed rows (`deleted_at IS NOT NULL`) are never touched —
    /// trash recovery is the explicit user contract for restoring
    /// deleted content and may genuinely hold empty buffers.
    pub fn purge_orphan_buffers(&self) -> Result<usize, Error> {
        // Treat zero-byte snapshots as "no content" — they're the
        // baseline snapshot the core thread writes for every
        // freshly-adopted empty buffer, not real content. Without
        // this every `tab.new` session would leave a snapshot row
        // behind that the sweep would refuse to clean up. Snapshots
        // belonging to orphan rows go away when their buffer row
        // does because the cascading delete picks them up via the
        // matching DELETE statement below.
        let n = self.conn.execute(
            "DELETE FROM buffers
             WHERE deleted_at IS NULL
               AND file_path IS NULL
               AND NOT EXISTS (SELECT 1 FROM buffer_edits e WHERE e.buffer_id = buffers.id)
               AND NOT EXISTS (
                    SELECT 1 FROM buffer_snapshots s
                     WHERE s.buffer_id = buffers.id AND s.byte_len > 0
               )",
            [],
        )?;
        // The cascading-delete tail: drop every zero-byte snapshot
        // whose owning buffers row was just removed. Without this
        // the snapshot rows survive as dangling references in the
        // `buffer_snapshots` table (no FK constraints in this
        // schema). The same pattern applies to the trash module's
        // `purge_expired`.
        self.conn.execute(
            "DELETE FROM buffer_snapshots
             WHERE NOT EXISTS (SELECT 1 FROM buffers b WHERE b.id = buffer_snapshots.buffer_id)",
            [],
        )?;
        self.conn.execute(
            "DELETE FROM buffer_edits
             WHERE NOT EXISTS (SELECT 1 FROM buffers b WHERE b.id = buffer_edits.buffer_id)",
            [],
        )?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn most_recent_buffer_picks_latest_touched() {
        let store = Store::open_in_memory().unwrap();
        let id_old = BufferId::new();
        let id_new = BufferId::new();
        store.upsert_buffer(id_old, 1_000, 1_000).unwrap();
        store.upsert_buffer(id_new, 2_000, 2_000).unwrap();
        store.touch_buffer(id_old, 500).unwrap(); // older than new
        let latest = store.most_recent_buffer().unwrap().unwrap();
        assert_eq!(latest, id_new);
    }

    #[test]
    fn most_recent_buffer_skips_deleted() {
        let store = Store::open_in_memory().unwrap();
        let id_a = BufferId::new();
        let id_b = BufferId::new();
        store.upsert_buffer(id_a, 1_000, 1_000).unwrap();
        store.upsert_buffer(id_b, 2_000, 2_000).unwrap();
        store.move_to_trash(id_b, 3_000, 30).unwrap();
        let latest = store.most_recent_buffer().unwrap().unwrap();
        assert_eq!(latest, id_a);
    }

    #[test]
    fn most_recent_buffer_none_when_empty() {
        let store = Store::open_in_memory().unwrap();
        assert!(store.most_recent_buffer().unwrap().is_none());
    }

    #[test]
    fn purge_orphan_buffers_removes_empty_no_file_rows() {
        let store = Store::open_in_memory().unwrap();
        let orphan = BufferId::new();
        let with_edit = BufferId::new();
        let with_snap = BufferId::new();
        store.upsert_buffer(orphan, 100, 100).unwrap();
        store.upsert_buffer(with_edit, 100, 100).unwrap();
        store.upsert_buffer(with_snap, 100, 100).unwrap();
        // Force a synthetic snapshot for `with_snap`.
        let buf =
            continuity_buffer::Buffer::from_parts(with_snap, "hi", continuity_buffer::Revision(0));
        store.save_snapshot(with_snap, &buf.snapshot()).unwrap();
        // Force an edit on `with_edit` via the codec helper.
        let op = continuity_text::EditOp::insert(continuity_text::Position::new(0, 0), "h");
        let row = crate::codec::encode_edit(
            with_edit,
            1,
            continuity_buffer::Revision(1),
            1_000,
            &op,
            None,
            &[],
            &[],
            None,
            0,
        );
        store.append_edit(&row).unwrap();
        let removed = store.purge_orphan_buffers().unwrap();
        assert_eq!(removed, 1);
        // The other two survive.
        let exists = |id: BufferId| -> bool {
            store
                .conn
                .query_row(
                    "SELECT 1 FROM buffers WHERE id = ?1",
                    params![id.as_uuid().as_bytes()],
                    |_| Ok(true),
                )
                .optional()
                .unwrap()
                .unwrap_or(false)
        };
        assert!(!exists(orphan));
        assert!(exists(with_edit));
        assert!(exists(with_snap));
    }

    #[test]
    fn purge_orphan_buffers_removes_rows_whose_only_snapshot_is_zero_byte() {
        // The core thread writes a baseline empty snapshot for
        // every freshly-adopted buffer; that alone should not
        // protect an otherwise-orphan row from being swept.
        let store = Store::open_in_memory().unwrap();
        let id = BufferId::new();
        store.upsert_buffer(id, 100, 100).unwrap();
        let buf = continuity_buffer::Buffer::from_parts(id, "", continuity_buffer::Revision(0));
        store.save_snapshot(id, &buf.snapshot()).unwrap();
        let removed = store.purge_orphan_buffers().unwrap();
        assert_eq!(removed, 1);
        // The dangling snapshot row is also cleaned up.
        let snap_count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM buffer_snapshots WHERE buffer_id = ?1",
                params![id.as_uuid().as_bytes()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(snap_count, 0);
    }

    #[test]
    fn purge_orphan_buffers_keeps_trashed_rows() {
        let store = Store::open_in_memory().unwrap();
        let trashed_empty = BufferId::new();
        store.upsert_buffer(trashed_empty, 100, 100).unwrap();
        store.move_to_trash(trashed_empty, 200, 30).unwrap();
        let removed = store.purge_orphan_buffers().unwrap();
        assert_eq!(removed, 0);
    }
}
