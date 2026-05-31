//! Undo-group row reads/writes for [`super::Store`].
//!
//! Owns the `undo_groups` table operations. The core thread mints new
//! groups at edit time and inserts them through here; recovery reloads
//! the full set in `ts` order to rebuild the in-memory undo tree.
//!
//! Thread ownership: the persistence thread (same as the parent
//! `Store`).

use continuity_buffer::{BufferId, UndoGroupId};
use rusqlite::params;

use super::{uuid_from_blob, Store, UndoGroupRow};
use crate::Error;

impl Store {
    /// Insert (or update on conflict) an undo-group row. Used both at edit
    /// time, when the core thread mints a new group, and at recovery time,
    /// when persisted rows are replayed.
    pub(crate) fn insert_undo_group(&self, row: &UndoGroupRow) -> Result<(), Error> {
        self.conn.execute(
            "INSERT INTO undo_groups (id, buffer_id, command_name, ts, parent_group_id)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
                 command_name = excluded.command_name,
                 ts = excluded.ts,
                 parent_group_id = excluded.parent_group_id",
            params![
                row.id.as_uuid().as_bytes(),
                row.buffer_id.as_uuid().as_bytes(),
                row.command_name,
                row.ts_ms,
                row.parent_group_id
                    .as_ref()
                    .map(|p| *p.as_uuid().as_bytes()),
            ],
        )?;
        Ok(())
    }

    /// Read every undo-group row belonging to `buffer_id`, in `ts` order.
    /// Used at recovery time to rebuild the in-memory undo tree.
    pub fn load_undo_groups(&self, buffer_id: BufferId) -> Result<Vec<UndoGroupRow>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id, command_name, ts, parent_group_id
             FROM undo_groups
             WHERE buffer_id = ?1
             ORDER BY ts ASC, id ASC",
        )?;
        let rows = stmt.query_map(params![buffer_id.as_uuid().as_bytes()], |r| {
            let id_blob: Vec<u8> = r.get(0)?;
            let parent_blob: Option<Vec<u8>> = r.get(3)?;
            let id = uuid_from_blob(&id_blob)
                .map(UndoGroupId::from_uuid)
                .ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Blob,
                        Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "undo_groups.id is not a 16-byte uuid",
                        )),
                    )
                })?;
            let parent_group_id = parent_blob
                .as_deref()
                .and_then(uuid_from_blob)
                .map(UndoGroupId::from_uuid);
            Ok(UndoGroupRow {
                id,
                buffer_id,
                command_name: r.get(1)?,
                ts_ms: r.get(2)?,
                parent_group_id,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_groups_round_trip() {
        let store = Store::open_in_memory().unwrap();
        let buffer_id = BufferId::new();
        let g1 = UndoGroupId::new();
        let g2 = UndoGroupId::new();
        store
            .insert_undo_group(&UndoGroupRow {
                id: g1,
                buffer_id,
                command_name: "editor.insert_char".into(),
                ts_ms: 100,
                parent_group_id: None,
            })
            .unwrap();
        store
            .insert_undo_group(&UndoGroupRow {
                id: g2,
                buffer_id,
                command_name: "editor.delete_back".into(),
                ts_ms: 200,
                parent_group_id: Some(g1),
            })
            .unwrap();
        let rows = store.load_undo_groups(buffer_id).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, g1);
        assert!(rows[0].parent_group_id.is_none());
        assert_eq!(rows[1].id, g2);
        assert_eq!(rows[1].parent_group_id, Some(g1));
        assert_eq!(rows[1].command_name, "editor.delete_back");
    }
}
