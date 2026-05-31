//! Edit-log append, replay, sequence numbering, and post-snapshot
//! pruning for [`super::Store`].
//!
//! Owns the `buffer_edits` table operations. Recovery consumers read
//! through [`Store::edits_since`]; the persistence thread appends one
//! row per accepted edit via [`Store::append_edit`]; the snapshot
//! policy invokes [`Store::prune_edits_at_or_before`] once a snapshot
//! covers an older revision range.
//!
//! Thread ownership: the persistence thread (same as the parent
//! `Store`).

use continuity_buffer::{BufferId, Revision};
use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

use super::{EditRow, Store};
use crate::Error;

impl Store {
    /// Append one row to the edit log. Caller is responsible for assigning
    /// `seq` (typically `1 + last seq for buffer_id`).
    pub fn append_edit(&self, edit: &EditRow) -> Result<(), Error> {
        self.conn.execute(
            "INSERT INTO buffer_edits
             (buffer_id, seq, revision, ts, op_kind,
              range_start_line, range_start_byte, range_end_line, range_end_byte,
              inserted_text, removed_text,
              selections_before_json, selections_after_json,
              undo_group_id, checksum_after)
             VALUES (?1, ?2, ?3, ?4, ?5,
                     ?6, ?7, ?8, ?9,
                     ?10, ?11,
                     ?12, ?13,
                     ?14, ?15)",
            params![
                edit.buffer_id.as_uuid().as_bytes(),
                edit.seq as i64,
                edit.revision.get() as i64,
                edit.ts_ms,
                edit.op_kind,
                edit.range_start_line.map(i64::from),
                edit.range_start_byte.map(i64::from),
                edit.range_end_line.map(i64::from),
                edit.range_end_byte.map(i64::from),
                edit.inserted_text,
                edit.removed_text,
                edit.selections_before_json,
                edit.selections_after_json,
                edit.undo_group_id.as_ref().map(Uuid::as_bytes),
                edit.checksum_after as i64,
            ],
        )?;
        Ok(())
    }

    /// Read every edit row for `buffer_id` whose revision is greater than
    /// `after_revision`, in `seq` order.
    pub fn edits_since(
        &self,
        buffer_id: BufferId,
        after_revision: Revision,
    ) -> Result<Vec<EditRow>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT seq, revision, ts, op_kind,
                    range_start_line, range_start_byte, range_end_line, range_end_byte,
                    inserted_text, removed_text,
                    selections_before_json, selections_after_json,
                    undo_group_id, checksum_after
             FROM buffer_edits
             WHERE buffer_id = ?1 AND revision > ?2
             ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map(
            params![buffer_id.as_uuid().as_bytes(), after_revision.get() as i64],
            |r| {
                let undo_blob: Option<Vec<u8>> = r.get(12)?;
                let undo_group_id = undo_blob.and_then(|b| {
                    <[u8; 16]>::try_from(b.as_slice())
                        .ok()
                        .map(Uuid::from_bytes)
                });
                Ok(EditRow {
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
        let mut edits = Vec::new();
        for row in rows {
            edits.push(row?);
        }
        Ok(edits)
    }

    /// The next available `seq` for a buffer (1 + max existing).
    pub fn next_seq(&self, buffer_id: BufferId) -> Result<u64, Error> {
        let max: Option<i64> = self
            .conn
            .query_row(
                "SELECT MAX(seq) FROM buffer_edits WHERE buffer_id = ?1",
                params![buffer_id.as_uuid().as_bytes()],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok(max.map(|n| n as u64 + 1).unwrap_or(1))
    }

    /// Drop edit rows for `buffer_id` whose `revision <= at_or_before`.
    ///
    /// Called after a snapshot has been written at `at_or_before`; once a
    /// snapshot covers a revision, the edit log entries leading up to it are
    /// no longer needed for recovery.
    pub(crate) fn prune_edits_at_or_before(
        &self,
        buffer_id: BufferId,
        at_or_before: Revision,
    ) -> Result<usize, Error> {
        let n = self.conn.execute(
            "DELETE FROM buffer_edits WHERE buffer_id = ?1 AND revision <= ?2",
            params![buffer_id.as_uuid().as_bytes(), at_or_before.get() as i64],
        )?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_log_round_trip() {
        let store = Store::open_in_memory().unwrap();
        let buffer_id = BufferId::new();

        for i in 0..3u64 {
            let row = EditRow {
                buffer_id,
                seq: i + 1,
                revision: Revision(i + 1),
                ts_ms: 1_700_000_000_000 + i as i64,
                op_kind: "insert".into(),
                range_start_line: Some(0),
                range_start_byte: Some(i as u32),
                range_end_line: None,
                range_end_byte: None,
                inserted_text: Some(format!("c{i}")),
                removed_text: None,
                selections_before_json: None,
                selections_after_json: None,
                undo_group_id: None,
                checksum_after: i,
            };
            store.append_edit(&row).unwrap();
        }

        let edits = store.edits_since(buffer_id, Revision(0)).unwrap();
        assert_eq!(edits.len(), 3);
        assert_eq!(edits[2].inserted_text.as_deref(), Some("c2"));

        let after_one = store.edits_since(buffer_id, Revision(1)).unwrap();
        assert_eq!(after_one.len(), 2);
    }

    #[test]
    fn next_seq_starts_at_one() {
        let store = Store::open_in_memory().unwrap();
        let id = BufferId::new();
        assert_eq!(store.next_seq(id).unwrap(), 1);
    }

    #[test]
    fn next_seq_increments_after_insert() {
        let store = Store::open_in_memory().unwrap();
        let id = BufferId::new();
        let row = EditRow {
            buffer_id: id,
            seq: 1,
            revision: Revision(1),
            ts_ms: 0,
            op_kind: "insert".into(),
            range_start_line: Some(0),
            range_start_byte: Some(0),
            range_end_line: None,
            range_end_byte: None,
            inserted_text: Some("x".into()),
            removed_text: None,
            selections_before_json: None,
            selections_after_json: None,
            undo_group_id: None,
            checksum_after: 0,
        };
        store.append_edit(&row).unwrap();
        assert_eq!(store.next_seq(id).unwrap(), 2);
    }

    #[test]
    fn prune_edits_at_or_before_drops_covered_rows() {
        let store = Store::open_in_memory().unwrap();
        let id = BufferId::new();
        for i in 1..=5u64 {
            let row = EditRow {
                buffer_id: id,
                seq: i,
                revision: Revision(i),
                ts_ms: i as i64,
                op_kind: "insert".into(),
                range_start_line: Some(0),
                range_start_byte: Some(0),
                range_end_line: None,
                range_end_byte: None,
                inserted_text: Some("x".into()),
                removed_text: None,
                selections_before_json: None,
                selections_after_json: None,
                undo_group_id: None,
                checksum_after: i,
            };
            store.append_edit(&row).unwrap();
        }
        let dropped = store.prune_edits_at_or_before(id, Revision(3)).unwrap();
        assert_eq!(dropped, 3);
        let remaining = store.edits_since(id, Revision(0)).unwrap();
        assert_eq!(remaining.len(), 2);
        assert_eq!(remaining[0].revision, Revision(4));
    }
}
