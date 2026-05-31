//! File-association persistence helpers for the existing `buffers.file_*`
//! columns.
//!
//! **Thread ownership**: only the persistence thread calls the free
//! functions in this module. Other threads use [`PersistClient`] methods,
//! which enqueue typed messages.

use std::path::PathBuf;

use continuity_buffer::{BufferId, FileAssociation};
use crossbeam_channel::bounded;
use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use crate::message::PersistMessage;
use crate::{Error, PersistClient};

type RawFileAssociationRow = (
    Option<String>,
    Option<i64>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
);

/// Persisted row-level file metadata for one buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferFileRow {
    /// Buffer id.
    pub buffer_id: BufferId,
    /// Associated file metadata, or `None` for ephemeral buffers.
    pub file: Option<FileAssociation>,
}

/// Update `buffers.file_path`, `file_mtime`, `file_hash`, and `file_content_hash`.
///
/// # Errors
///
/// Returns [`Error::Sqlite`] when the update fails.
pub fn set_buffer_file(
    conn: &Connection,
    buffer_id: BufferId,
    file: Option<&FileAssociation>,
) -> Result<(), Error> {
    match file {
        Some(file) => {
            conn.execute(
                "UPDATE buffers
                 SET file_path = ?2,
                     file_mtime = ?3,
                     file_hash = ?4,
                     file_content_hash = ?5
                 WHERE id = ?1",
                params![
                    buffer_id.as_uuid().as_bytes().as_slice(),
                    file.path.to_string_lossy().as_ref(),
                    file.mtime_ms,
                    file.hash.to_be_bytes().as_slice(),
                    file.content_hash.to_be_bytes().as_slice(),
                ],
            )?;
        }
        None => {
            conn.execute(
                "UPDATE buffers
                 SET file_path = NULL,
                     file_mtime = NULL,
                     file_hash = NULL,
                     file_content_hash = NULL
                 WHERE id = ?1",
                params![buffer_id.as_uuid().as_bytes().as_slice()],
            )?;
        }
    }
    Ok(())
}

/// Load file metadata for a buffer.
///
/// # Errors
///
/// Returns [`Error::Sqlite`] for query failures or [`Error::Decode`] for a
/// malformed hash blob.
pub fn load_buffer_file(
    conn: &Connection,
    buffer_id: BufferId,
) -> Result<Option<FileAssociation>, Error> {
    let row: Option<RawFileAssociationRow> = conn
        .query_row(
            "SELECT file_path, file_mtime, file_hash, file_content_hash
             FROM buffers
             WHERE id = ?1 AND deleted_at IS NULL",
            params![buffer_id.as_uuid().as_bytes().as_slice()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((Some(path), Some(mtime_ms), Some(hash_blob), content_hash_blob)) = row else {
        return Ok(None);
    };
    let hash = decode_hash(&hash_blob)?;
    let content_hash = content_hash_blob
        .as_deref()
        .map(decode_hash)
        .transpose()?
        .unwrap_or(hash);
    Ok(Some(
        FileAssociation::new(PathBuf::from(path), mtime_ms, hash).with_content_hash(content_hash),
    ))
}

/// Load every non-deleted buffer id.
///
/// # Errors
///
/// Returns [`Error::Sqlite`] for query failures or [`Error::Decode`] for a
/// malformed id blob.
pub fn load_active_buffer_ids(conn: &Connection) -> Result<Vec<BufferId>, Error> {
    let mut stmt = conn.prepare(
        "SELECT id FROM buffers
         WHERE deleted_at IS NULL
         ORDER BY last_touched DESC",
    )?;
    let rows = stmt.query_map([], |r| r.get::<_, Vec<u8>>(0))?;
    let mut out = Vec::new();
    for row in rows {
        let bytes = row?;
        out.push(decode_buffer_id(&bytes)?);
    }
    Ok(out)
}

fn decode_hash(bytes: &[u8]) -> Result<u64, Error> {
    if bytes.len() != 8 {
        return Err(Error::Decode(format!(
            "buffers.file_hash expected 8 bytes, got {}",
            bytes.len()
        )));
    }
    let mut arr = [0u8; 8];
    arr.copy_from_slice(bytes);
    Ok(u64::from_be_bytes(arr))
}

fn decode_buffer_id(bytes: &[u8]) -> Result<BufferId, Error> {
    if bytes.len() != 16 {
        return Err(Error::Decode(format!(
            "buffers.id expected 16 bytes, got {}",
            bytes.len()
        )));
    }
    let mut arr = [0u8; 16];
    arr.copy_from_slice(bytes);
    Ok(BufferId::from_uuid(Uuid::from_bytes(arr)))
}

impl PersistClient {
    /// Synchronously update a buffer's file association.
    ///
    /// # Errors
    ///
    /// Propagates any persistence-thread error, or [`Error::ThreadGone`].
    pub fn set_buffer_file(
        &self,
        buffer_id: BufferId,
        file: Option<FileAssociation>,
    ) -> Result<(), Error> {
        let (tx, rx) = bounded(1);
        self.sender()
            .send(PersistMessage::SetBufferFile {
                buffer_id,
                file,
                reply: Some(tx),
            })
            .map_err(|_| Error::ThreadGone)?;
        rx.recv().map_err(|_| Error::ThreadGone)?
    }

    /// Fire-and-forget update of a buffer's file association.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ThreadGone`] if the persistence thread has exited.
    pub fn set_buffer_file_async(
        &self,
        buffer_id: BufferId,
        file: Option<FileAssociation>,
    ) -> Result<(), Error> {
        self.sender()
            .send(PersistMessage::SetBufferFile {
                buffer_id,
                file,
                reply: None,
            })
            .map_err(|_| Error::ThreadGone)
    }

    /// Load one buffer's file association.
    ///
    /// # Errors
    ///
    /// Propagates any persistence-thread error, or [`Error::ThreadGone`].
    pub fn load_buffer_file(&self, buffer_id: BufferId) -> Result<Option<FileAssociation>, Error> {
        let (tx, rx) = bounded(1);
        self.sender()
            .send(PersistMessage::LoadBufferFile {
                buffer_id,
                reply: tx,
            })
            .map_err(|_| Error::ThreadGone)?;
        rx.recv().map_err(|_| Error::ThreadGone)?
    }

    /// Load every active buffer id.
    ///
    /// # Errors
    ///
    /// Propagates any persistence-thread error, or [`Error::ThreadGone`].
    pub fn load_active_buffer_ids(&self) -> Result<Vec<BufferId>, Error> {
        let (tx, rx) = bounded(1);
        self.sender()
            .send(PersistMessage::LoadActiveBufferIds { reply: tx })
            .map_err(|_| Error::ThreadGone)?;
        rx.recv().map_err(|_| Error::ThreadGone)?
    }

    /// Return the next available edit sequence number for a buffer.
    ///
    /// # Errors
    ///
    /// Propagates any persistence-thread error, or [`Error::ThreadGone`].
    pub fn next_seq(&self, buffer_id: BufferId) -> Result<u64, Error> {
        let (tx, rx) = bounded(1);
        self.sender()
            .send(PersistMessage::NextSeq {
                buffer_id,
                reply: tx,
            })
            .map_err(|_| Error::ThreadGone)?;
        rx.recv().map_err(|_| Error::ThreadGone)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn file_association_round_trips() {
        let store = Store::open_in_memory().unwrap();
        let id = BufferId::new();
        store.upsert_buffer(id, 1, 1).unwrap();
        let file = FileAssociation::new(PathBuf::from("C:\\tmp\\note.md"), 123, 0xCAFE);
        set_buffer_file(store.conn(), id, Some(&file)).unwrap();
        assert_eq!(load_buffer_file(store.conn(), id).unwrap(), Some(file));
    }

    #[test]
    fn clearing_file_association_round_trips() {
        let store = Store::open_in_memory().unwrap();
        let id = BufferId::new();
        store.upsert_buffer(id, 1, 1).unwrap();
        let file = FileAssociation::new(PathBuf::from("C:\\tmp\\note.md"), 123, 0xCAFE);
        set_buffer_file(store.conn(), id, Some(&file)).unwrap();
        set_buffer_file(store.conn(), id, None).unwrap();
        assert_eq!(load_buffer_file(store.conn(), id).unwrap(), None);
    }
}
