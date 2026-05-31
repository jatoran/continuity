//! Trash insertion (move to trash with retention expiry) and sweep
//! (purge expired rows + cascade to snapshots / edits / undo groups)
//! for [`super::Store`].
//!
//! Owns the `trash` table operations and the cascade `DELETE`s that
//! permanently remove a purged buffer. After a purge runs, a
//! `wal_checkpoint(TRUNCATE)` reclaims WAL space (best-effort).
//!
//! Thread ownership: the persistence thread (same as the parent
//! `Store`).

use continuity_buffer::BufferId;
use rusqlite::params;

use super::{Store, ONE_DAY_MS};
use crate::Error;

impl Store {
    /// Mark a buffer as deleted and record an expiry in the `trash` table.
    ///
    /// `retention_days = 0` means immediate eligibility for purge.
    pub fn move_to_trash(
        &self,
        buffer_id: BufferId,
        now_ms: i64,
        retention_days: u32,
    ) -> Result<(), Error> {
        let expires_at = now_ms.saturating_add(i64::from(retention_days) * ONE_DAY_MS);
        let uuid = buffer_id.as_uuid();
        let id_bytes = uuid.as_bytes();
        self.conn.execute(
            "UPDATE buffers SET deleted_at = ?2 WHERE id = ?1",
            params![id_bytes, now_ms],
        )?;
        self.conn.execute(
            "INSERT INTO trash (buffer_id, deleted_at, expires_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(buffer_id) DO UPDATE SET
                 deleted_at = excluded.deleted_at,
                 expires_at = excluded.expires_at",
            params![id_bytes, now_ms, expires_at],
        )?;
        Ok(())
    }

    /// Hard-purge every trash entry whose `expires_at <= now_ms`. Returns the
    /// number of buffers purged.
    ///
    /// Removes the trash row, the buffers row, all snapshots, all edits, and
    /// any related undo groups. After purging, runs
    /// `PRAGMA wal_checkpoint(TRUNCATE)` so the on-disk WAL doesn't grow
    /// indefinitely.
    pub fn purge_expired(&self, now_ms: i64) -> Result<usize, Error> {
        let expired_ids: Vec<Vec<u8>> = {
            let mut stmt = self
                .conn
                .prepare("SELECT buffer_id FROM trash WHERE expires_at <= ?1")?;
            let rows = stmt.query_map(params![now_ms], |r| r.get::<_, Vec<u8>>(0))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            out
        };
        if expired_ids.is_empty() {
            return Ok(0);
        }
        for id in &expired_ids {
            self.conn
                .execute("DELETE FROM buffer_edits WHERE buffer_id = ?1", params![id])?;
            self.conn.execute(
                "DELETE FROM buffer_snapshots WHERE buffer_id = ?1",
                params![id],
            )?;
            self.conn
                .execute("DELETE FROM undo_groups WHERE buffer_id = ?1", params![id])?;
            self.conn
                .execute("DELETE FROM buffers WHERE id = ?1", params![id])?;
            self.conn
                .execute("DELETE FROM trash WHERE buffer_id = ?1", params![id])?;
        }
        // Reclaim WAL space after a purge. Failure is non-fatal.
        let _ = self.conn.pragma_update(None, "wal_checkpoint", "TRUNCATE");
        Ok(expired_ids.len())
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::OptionalExtension;

    use super::*;

    #[test]
    fn purge_expired_drops_expired_only() {
        let store = Store::open_in_memory().unwrap();
        let id_keep = BufferId::new();
        let id_expire = BufferId::new();
        store.upsert_buffer(id_keep, 100, 100).unwrap();
        store.upsert_buffer(id_expire, 100, 100).unwrap();
        store.move_to_trash(id_keep, 200, 30).unwrap();
        store.move_to_trash(id_expire, 200, 0).unwrap();
        // 30 days into the future — only id_expire's expiry has passed
        // (retention 0 = expires immediately).
        let purged = store.purge_expired(300).unwrap();
        assert_eq!(purged, 1);
        // id_expire row should be gone from buffers entirely.
        let row: Option<Vec<u8>> = store
            .conn()
            .query_row(
                "SELECT id FROM buffers WHERE id = ?1",
                params![id_expire.as_uuid().as_bytes()],
                |r| r.get(0),
            )
            .optional()
            .unwrap();
        assert!(row.is_none());
        // id_keep still present.
        let row: Option<Vec<u8>> = store
            .conn()
            .query_row(
                "SELECT id FROM buffers WHERE id = ?1",
                params![id_keep.as_uuid().as_bytes()],
                |r| r.get(0),
            )
            .optional()
            .unwrap();
        assert!(row.is_some());
    }
}
