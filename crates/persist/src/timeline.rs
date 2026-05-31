//! Phase I1 — buffer timeline persistence helpers: list snapshot
//! summaries for the drag-slider, and stamp / clear the user-supplied
//! label on a snapshot row.
//!
//! Thread ownership: every function takes `&Store`, whose connection
//! is single-threaded (the persistence thread). The functions are
//! invoked from the persist loop in response to dedicated
//! [`crate::PersistMessage`] variants.
//!
//! The persistence schema for the label column is owned by
//! [`crate::schema::SCHEMA_V4`] — see that migration for the
//! `buffer_snapshots.label` rationale.

use continuity_buffer::{Buffer, BufferId, Revision};
use rusqlite::{params, OptionalExtension};

use crate::codec::decode_op;
use crate::store::{SnapshotRow, SnapshotSummaryRow, Store};
use crate::Error;

/// Tuple of columns read from `buffer_snapshots` by
/// [`Store::load_snapshot_at_or_before`]: `(id, revision, content_blob,
/// byte_len, line_count, checksum, label)`.
type SnapshotAtRevisionRow = (i64, i64, Vec<u8>, i64, i64, i64, Option<String>);

impl Store {
    /// Phase I1: stamp `label` onto the snapshot row at `revision`. Pass
    /// `None` to clear the label. Returns the number of rows affected
    /// (0 if no snapshot exists at that revision yet — used by the
    /// "label the next snapshot" flow where the UI defers the call
    /// until the snapshot is actually written).
    ///
    /// # Errors
    ///
    /// Propagates any [`Error::Sqlite`].
    pub fn set_snapshot_label(
        &self,
        buffer_id: BufferId,
        revision: Revision,
        label: Option<&str>,
    ) -> Result<usize, Error> {
        let n = self.conn().execute(
            "UPDATE buffer_snapshots
             SET label = ?3
             WHERE buffer_id = ?1 AND revision = ?2",
            params![buffer_id.as_uuid().as_bytes(), revision.get() as i64, label,],
        )?;
        Ok(n)
    }

    /// Phase I1: list every snapshot for `buffer_id` as a lightweight
    /// summary (no decompression). Used to populate the timeline
    /// slider's tick marks (named snapshot vs. edit-only revision)
    /// and per-tick tooltip metadata.
    ///
    /// Ordered ascending by revision for deterministic display.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error::Sqlite`].
    pub fn list_snapshot_summaries(
        &self,
        buffer_id: BufferId,
    ) -> Result<Vec<SnapshotSummaryRow>, Error> {
        let mut stmt = self.conn().prepare(
            "SELECT revision, created_at, label
             FROM buffer_snapshots
             WHERE buffer_id = ?1
             ORDER BY revision ASC, id ASC",
        )?;
        let rows = stmt.query_map(params![buffer_id.as_uuid().as_bytes()], |r| {
            Ok(SnapshotSummaryRow {
                revision: Revision(r.get::<_, i64>(0)? as u64),
                created_at_ms: r.get(1)?,
                label: r.get::<_, Option<String>>(2)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Phase I1: materialize a buffer's rope content at `target_revision`
    /// by loading the latest snapshot at-or-before `target_revision`
    /// and replaying every persisted edit row up through
    /// `target_revision` on top of it.
    ///
    /// Returns `Ok(None)` when the buffer has no snapshot at-or-before
    /// the target revision (e.g. the slider is dragged earlier than
    /// the earliest persisted snapshot).
    ///
    /// Read-only: never writes to the database. Used by the
    /// time-machine slider to preview a past revision in the active
    /// pane without persistence side effects.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error::Sqlite`] from snapshot or edit lookups,
    /// and [`Error::Codec`] when an edit row cannot be decoded.
    pub fn load_content_at_revision(
        &self,
        buffer_id: BufferId,
        target_revision: Revision,
    ) -> Result<Option<String>, Error> {
        let Some(snap) = self.load_snapshot_at_or_before(buffer_id, target_revision)? else {
            return Ok(None);
        };
        if snap.revision == target_revision {
            return Ok(Some(snap.content));
        }
        let mut buf = Buffer::from_parts(buffer_id, &snap.content, snap.revision);
        for row in self.edits_in_revision_range(buffer_id, snap.revision, target_revision)? {
            let op = decode_op(&row)?;
            buf.apply(&op).map_err(|e| {
                Error::Decode(format!(
                    "edit at seq {} failed to apply during time-machine load: {e}",
                    row.seq
                ))
            })?;
        }
        Ok(Some(buf.rope().to_string()))
    }

    /// Phase I1: load the latest snapshot row whose revision is at-or-
    /// before `target_revision`, decompressing the content blob and
    /// verifying its FNV-1a checksum. Helper for
    /// [`Self::load_content_at_revision`]; not part of the public
    /// snapshot-recovery contract (see
    /// [`Self::load_latest_valid_snapshot`] for the recovery path).
    fn load_snapshot_at_or_before(
        &self,
        buffer_id: BufferId,
        target_revision: Revision,
    ) -> Result<Option<SnapshotRow>, Error> {
        let row: Option<SnapshotAtRevisionRow> = self
            .conn()
            .query_row(
                "SELECT id, revision, content_blob, byte_len, line_count, checksum, label
                 FROM buffer_snapshots
                 WHERE buffer_id = ?1 AND revision <= ?2
                 ORDER BY revision DESC, id DESC
                 LIMIT 1",
                params![buffer_id.as_uuid().as_bytes(), target_revision.get() as i64],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .optional()?;
        let Some((id, revision, blob, byte_len, line_count, checksum, label)) = row else {
            return Ok(None);
        };
        let bytes = zstd::stream::decode_all(blob.as_slice())?;
        let stored_checksum = checksum as u64;
        if crate::checksum::fnv1a_64(&bytes) != stored_checksum {
            return Err(Error::ChecksumMismatch {
                revision: revision as u64,
            });
        }
        let content = String::from_utf8(bytes).map_err(|e| {
            Error::Compression(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
        })?;
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

    /// Phase I1: load every persisted edit row for `buffer_id` whose
    /// revision is in `(after_revision, target_revision]`, ordered by
    /// seq ascending. Helper for [`Self::load_content_at_revision`].
    fn edits_in_revision_range(
        &self,
        buffer_id: BufferId,
        after_revision: Revision,
        target_revision: Revision,
    ) -> Result<Vec<crate::store::EditRow>, Error> {
        let mut stmt = self.conn().prepare(
            "SELECT seq, revision, ts, op_kind,
                    range_start_line, range_start_byte, range_end_line, range_end_byte,
                    inserted_text, removed_text,
                    selections_before_json, selections_after_json,
                    undo_group_id, checksum_after
             FROM buffer_edits
             WHERE buffer_id = ?1 AND revision > ?2 AND revision <= ?3
             ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map(
            params![
                buffer_id.as_uuid().as_bytes(),
                after_revision.get() as i64,
                target_revision.get() as i64,
            ],
            |r| {
                let undo_blob: Option<Vec<u8>> = r.get(12)?;
                let undo_group_id = undo_blob.and_then(|b| {
                    <[u8; 16]>::try_from(b.as_slice())
                        .ok()
                        .map(uuid::Uuid::from_bytes)
                });
                Ok(crate::store::EditRow {
                    buffer_id,
                    seq: r.get::<_, i64>(0)? as u64,
                    revision: Revision(r.get::<_, i64>(1)? as u64),
                    ts_ms: r.get(2)?,
                    op_kind: r.get(3)?,
                    range_start_line: r.get::<_, Option<i64>>(4)?.map(|n| n as u32),
                    range_start_byte: r.get::<_, Option<i64>>(5)?.map(|n| n as u32),
                    range_end_line: r.get::<_, Option<i64>>(6)?.map(|n| n as u32),
                    range_end_byte: r.get::<_, Option<i64>>(7)?.map(|n| n as u32),
                    inserted_text: r.get(8)?,
                    removed_text: r.get(9)?,
                    selections_before_json: r.get(10)?,
                    selections_after_json: r.get(11)?,
                    undo_group_id,
                    checksum_after: r.get::<_, i64>(13)? as u64,
                })
            },
        )?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Phase I1: look up a snapshot summary at an exact revision.
    /// Returns `None` when no snapshot exists at that revision.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error::Sqlite`].
    pub fn snapshot_summary_at(
        &self,
        buffer_id: BufferId,
        revision: Revision,
    ) -> Result<Option<SnapshotSummaryRow>, Error> {
        let row = self
            .conn()
            .query_row(
                "SELECT revision, created_at, label
                 FROM buffer_snapshots
                 WHERE buffer_id = ?1 AND revision = ?2
                 LIMIT 1",
                params![buffer_id.as_uuid().as_bytes(), revision.get() as i64],
                |r| {
                    Ok(SnapshotSummaryRow {
                        revision: Revision(r.get::<_, i64>(0)? as u64),
                        created_at_ms: r.get(1)?,
                        label: r.get::<_, Option<String>>(2)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }
}

#[cfg(test)]
mod tests {
    use continuity_buffer::Buffer;
    use continuity_text::{EditOp, Position};

    use crate::store::Store;

    #[test]
    fn list_summaries_returns_rows_in_revision_order() {
        let store = Store::open_in_memory().unwrap();
        let mut buf = Buffer::from_text("a");
        store.save_snapshot(buf.id(), &buf.snapshot()).unwrap();
        buf.apply(&EditOp::insert(Position::new(0, 1), "b"))
            .unwrap();
        store.save_snapshot(buf.id(), &buf.snapshot()).unwrap();
        buf.apply(&EditOp::insert(Position::new(0, 2), "c"))
            .unwrap();
        store.save_snapshot(buf.id(), &buf.snapshot()).unwrap();

        let summaries = store.list_snapshot_summaries(buf.id()).unwrap();
        assert_eq!(summaries.len(), 3);
        // Ascending order.
        assert!(summaries[0].revision <= summaries[1].revision);
        assert!(summaries[1].revision <= summaries[2].revision);
        // No labels yet.
        assert!(summaries.iter().all(|s| s.label.is_none()));
    }

    #[test]
    fn set_label_round_trip() {
        let store = Store::open_in_memory().unwrap();
        let buf = Buffer::from_text("hello");
        store.save_snapshot(buf.id(), &buf.snapshot()).unwrap();
        let rev = buf.snapshot().revision();

        let n = store
            .set_snapshot_label(buf.id(), rev, Some("draft 1"))
            .unwrap();
        assert_eq!(n, 1);

        let s = store.snapshot_summary_at(buf.id(), rev).unwrap().unwrap();
        assert_eq!(s.label.as_deref(), Some("draft 1"));

        // Clearing the label sets it back to NULL.
        let n2 = store.set_snapshot_label(buf.id(), rev, None).unwrap();
        assert_eq!(n2, 1);
        let s2 = store.snapshot_summary_at(buf.id(), rev).unwrap().unwrap();
        assert!(s2.label.is_none());
    }

    #[test]
    fn set_label_on_missing_revision_returns_zero() {
        let store = Store::open_in_memory().unwrap();
        let buf = Buffer::from_text("h");
        let n = store
            .set_snapshot_label(buf.id(), continuity_buffer::Revision(99), Some("x"))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn snapshot_summary_at_missing_returns_none() {
        let store = Store::open_in_memory().unwrap();
        let buf = Buffer::from_text("h");
        let s = store
            .snapshot_summary_at(buf.id(), continuity_buffer::Revision(7))
            .unwrap();
        assert!(s.is_none());
    }

    #[test]
    fn load_content_at_revision_replays_edits_on_top_of_snapshot() {
        use crate::codec::encode_edit;
        let store = Store::open_in_memory().unwrap();
        let mut buf = Buffer::from_text("a");
        let id = buf.id();
        store.save_snapshot(id, &buf.snapshot()).unwrap();
        // Apply two edits without snapshotting again, so the snapshot
        // row stays at revision 0 and the content must be reconstructed
        // by replaying both edits.
        let op_b = EditOp::insert(Position::new(0, 1), "b");
        buf.apply(&op_b).unwrap();
        let after_b = buf.revision();
        let checksum_b = crate::checksum::fnv1a_64_chunks(buf.rope().chunks().map(str::as_bytes));
        store
            .append_edit(&encode_edit(
                id,
                1,
                after_b,
                1_000,
                &op_b,
                None,
                &[],
                &[],
                None,
                checksum_b,
            ))
            .unwrap();
        let op_c = EditOp::insert(Position::new(0, 2), "c");
        buf.apply(&op_c).unwrap();
        let after_c = buf.revision();
        let checksum_c = crate::checksum::fnv1a_64_chunks(buf.rope().chunks().map(str::as_bytes));
        store
            .append_edit(&encode_edit(
                id,
                2,
                after_c,
                2_000,
                &op_c,
                None,
                &[],
                &[],
                None,
                checksum_c,
            ))
            .unwrap();

        // At head: full content.
        let head = store
            .load_content_at_revision(id, after_c)
            .unwrap()
            .unwrap();
        assert_eq!(head, "abc");
        // One edit back.
        let mid = store
            .load_content_at_revision(id, after_b)
            .unwrap()
            .unwrap();
        assert_eq!(mid, "ab");
        // At the snapshot revision itself.
        let base = store
            .load_content_at_revision(id, continuity_buffer::Revision(0))
            .unwrap()
            .unwrap();
        assert_eq!(base, "a");
    }

    #[test]
    fn load_content_at_revision_returns_none_for_unknown_buffer() {
        let store = Store::open_in_memory().unwrap();
        let result = store
            .load_content_at_revision(
                continuity_buffer::BufferId::new(),
                continuity_buffer::Revision(0),
            )
            .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn label_persists_through_load_latest() {
        let store = Store::open_in_memory().unwrap();
        let buf = Buffer::from_text("hi");
        store.save_snapshot(buf.id(), &buf.snapshot()).unwrap();
        store
            .set_snapshot_label(buf.id(), buf.snapshot().revision(), Some("pre-refactor"))
            .unwrap();

        let row = store.load_latest_snapshot(buf.id()).unwrap().unwrap();
        assert_eq!(row.label.as_deref(), Some("pre-refactor"));
    }
}
