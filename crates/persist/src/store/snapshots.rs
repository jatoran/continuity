//! Snapshot writes, reads, and corruption-tolerant fallback for the
//! persistence [`super::Store`].
//!
//! Owns the `buffer_snapshots` table operations: zstd encode/decode,
//! FNV-1a checksum verify, and the descending-revision fallback walk
//! that recovery relies on when the most recent blob fails to
//! round-trip.
//!
//! Thread ownership: the persistence thread (same as the parent
//! `Store`).

use continuity_buffer::{BufferId, Revision, RopeSnapshot};
use rusqlite::{params, OptionalExtension};

use super::{unix_ms_now, SnapshotRow, Store};
use crate::{checksum::fnv1a_64, Error};

impl Store {
    /// Persist a [`RopeSnapshot`] for `buffer_id`. Returns the assigned id.
    pub fn save_snapshot(
        &self,
        buffer_id: BufferId,
        snapshot: &RopeSnapshot,
    ) -> Result<i64, Error> {
        let content = snapshot.rope().to_string();
        let bytes = content.as_bytes();
        let checksum = fnv1a_64(bytes);
        let compressed = zstd::stream::encode_all(bytes, 3)?;
        let now = unix_ms_now();
        let line_count = u32::try_from(snapshot.rope().len_lines()).unwrap_or(u32::MAX);

        // Upsert the buffers row (no-op if it exists).
        self.conn.execute(
            "INSERT OR IGNORE INTO buffers (id, created_at, last_touched, current_revision)
             VALUES (?1, ?2, ?2, ?3)",
            params![
                buffer_id.as_uuid().as_bytes(),
                now,
                snapshot.revision().get() as i64
            ],
        )?;

        self.conn.execute(
            "INSERT INTO buffer_snapshots
             (buffer_id, revision, created_at, content_blob, content_codec,
              byte_len, line_count, checksum)
             VALUES (?1, ?2, ?3, ?4, 'zstd', ?5, ?6, ?7)",
            params![
                buffer_id.as_uuid().as_bytes(),
                snapshot.revision().get() as i64,
                now,
                compressed,
                bytes.len() as i64,
                line_count as i64,
                checksum as i64,
            ],
        )?;
        let snapshot_id = self.conn.last_insert_rowid();

        self.conn.execute(
            "UPDATE buffers
             SET last_touched = ?2,
                 current_snapshot_id = ?3,
                 current_revision = ?4
             WHERE id = ?1",
            params![
                buffer_id.as_uuid().as_bytes(),
                now,
                snapshot_id,
                snapshot.revision().get() as i64,
            ],
        )?;

        Ok(snapshot_id)
    }

    /// Load the most recent snapshot for `buffer_id`, or `None` if there is
    /// none.
    pub fn load_latest_snapshot(&self, buffer_id: BufferId) -> Result<Option<SnapshotRow>, Error> {
        let row: Option<(i64, i64, Vec<u8>, i64, i64, i64)> = self
            .conn
            .query_row(
                "SELECT id, revision, content_blob, byte_len, line_count, checksum
                 FROM buffer_snapshots
                 WHERE buffer_id = ?1
                 ORDER BY revision DESC
                 LIMIT 1",
                params![buffer_id.as_uuid().as_bytes()],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .optional()?;

        let Some((id, revision, blob, byte_len, line_count, checksum)) = row else {
            return Ok(None);
        };

        let bytes = zstd::stream::decode_all(blob.as_slice())?;
        let stored_checksum = checksum as u64;
        if fnv1a_64(&bytes) != stored_checksum {
            return Err(Error::ChecksumMismatch {
                revision: revision as u64,
            });
        }
        let content = String::from_utf8(bytes).map_err(|e| {
            Error::Compression(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
        })?;

        // Pull the label as a second cheap query (the hot recovery path
        // doesn't need it; making this a separate read keeps the original
        // 6-column `SELECT` unchanged for older callsites and recovery
        // paths that already mapped the row positionally).
        let label: Option<String> = self
            .conn
            .query_row(
                "SELECT label FROM buffer_snapshots WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();

        Ok(Some(SnapshotRow {
            id: Some(id),
            buffer_id,
            revision: Revision(revision as u64),
            content,
            byte_len: byte_len as u64,
            line_count: line_count as u32,
            checksum: stored_checksum,
            label,
            created_at_ms: 0,
        }))
    }

    /// The most-recent valid snapshot for `buffer_id`. Walks snapshots in
    /// descending revision order, decompressing and verifying the FNV-1a
    /// checksum at each step. Returns the first one that round-trips
    /// cleanly, or `None` when there are no snapshots (or none are valid).
    ///
    /// Corrupt snapshots are logged to stderr and skipped — this is the
    /// fallback path the spec calls for in §4 ("if corrupt, fall back to the
    /// previous snapshot").
    pub(crate) fn load_latest_valid_snapshot(
        &self,
        buffer_id: BufferId,
    ) -> Result<Option<SnapshotRow>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id, revision, content_blob, byte_len, line_count, checksum, label
             FROM buffer_snapshots
             WHERE buffer_id = ?1
             ORDER BY revision DESC, id DESC",
        )?;
        let rows = stmt.query_map(params![buffer_id.as_uuid().as_bytes()], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Vec<u8>>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })?;
        for row in rows {
            let (id, revision, blob, byte_len, line_count, checksum, label) = row?;
            match decompress_and_verify(&blob, checksum as u64) {
                Ok(content) => {
                    return Ok(Some(SnapshotRow {
                        id: Some(id),
                        buffer_id,
                        revision: Revision(revision as u64),
                        content,
                        byte_len: byte_len as u64,
                        line_count: line_count as u32,
                        checksum: checksum as u64,
                        label,
                        created_at_ms: 0,
                    }));
                }
                Err(e) => {
                    eprintln!(
                        "continuity-persist: snapshot id={id} rev={revision} corrupt: {e}; falling back"
                    );
                }
            }
        }
        Ok(None)
    }
}

/// Decompress a zstd snapshot blob and verify its FNV-1a checksum and
/// UTF-8-ness. Returns the decoded text on success.
fn decompress_and_verify(blob: &[u8], stored_checksum: u64) -> Result<String, Error> {
    let bytes = zstd::stream::decode_all(blob)?;
    if fnv1a_64(&bytes) != stored_checksum {
        return Err(Error::ChecksumMismatch { revision: 0 });
    }
    String::from_utf8(bytes)
        .map_err(|e| Error::Compression(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))
}

#[cfg(test)]
mod tests {
    use continuity_buffer::Buffer;
    use continuity_text::{EditOp, Position};

    use super::*;

    #[test]
    fn snapshot_round_trip() {
        let store = Store::open_in_memory().unwrap();
        let mut buf = Buffer::from_text("hello world");
        buf.apply(&EditOp::insert(Position::new(0, 11), "!"))
            .unwrap();
        let snap = buf.snapshot();

        let id = store.save_snapshot(buf.id(), &snap).unwrap();
        assert!(id > 0);

        let loaded = store.load_latest_snapshot(buf.id()).unwrap().unwrap();
        assert_eq!(loaded.content, "hello world!");
        assert_eq!(loaded.revision, snap.revision());
        assert_eq!(loaded.byte_len, 12);
    }

    #[test]
    fn latest_snapshot_returns_highest_revision() {
        let store = Store::open_in_memory().unwrap();
        let mut buf = Buffer::from_text("a");
        let s1 = buf.snapshot();
        store.save_snapshot(buf.id(), &s1).unwrap();
        buf.apply(&EditOp::insert(Position::new(0, 1), "b"))
            .unwrap();
        let s2 = buf.snapshot();
        store.save_snapshot(buf.id(), &s2).unwrap();

        let loaded = store.load_latest_snapshot(buf.id()).unwrap().unwrap();
        assert_eq!(loaded.content, "ab");
        assert_eq!(loaded.revision, Revision(1));
    }

    #[test]
    fn missing_buffer_returns_none() {
        let store = Store::open_in_memory().unwrap();
        let id = BufferId::new();
        assert!(store.load_latest_snapshot(id).unwrap().is_none());
    }

    #[test]
    fn checksum_mismatch_detected() {
        // Round-trip a snapshot, then corrupt the stored blob.
        let store = Store::open_in_memory().unwrap();
        let buf = Buffer::from_text("hello");
        store.save_snapshot(buf.id(), &buf.snapshot()).unwrap();
        store
            .conn()
            .execute("UPDATE buffer_snapshots SET checksum = checksum + 1", [])
            .unwrap();
        let err = store.load_latest_snapshot(buf.id()).unwrap_err();
        assert!(matches!(err, Error::ChecksumMismatch { .. }));
    }

    #[test]
    fn load_latest_valid_snapshot_falls_back_on_corruption() {
        let store = Store::open_in_memory().unwrap();
        let mut buf = Buffer::from_text("a");
        store.save_snapshot(buf.id(), &buf.snapshot()).unwrap();
        buf.apply(&EditOp::insert(Position::new(0, 1), "b"))
            .unwrap();
        store.save_snapshot(buf.id(), &buf.snapshot()).unwrap();
        // Corrupt only the most-recent snapshot.
        store
            .conn()
            .execute(
                "UPDATE buffer_snapshots SET checksum = checksum + 1
                 WHERE id = (SELECT MAX(id) FROM buffer_snapshots)",
                [],
            )
            .unwrap();
        let snap = store
            .load_latest_valid_snapshot(buf.id())
            .unwrap()
            .expect("fallback present");
        assert_eq!(snap.content, "a"); // fell back to the older valid one
        assert_eq!(snap.revision, Revision(0));
    }

    #[test]
    fn load_latest_valid_snapshot_none_when_no_snapshots() {
        let store = Store::open_in_memory().unwrap();
        assert!(store
            .load_latest_valid_snapshot(BufferId::new())
            .unwrap()
            .is_none());
    }
}
