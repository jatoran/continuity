//! Per-window persistent state — the `windows` table introduced by
//! schema v3 (Phase 14).
//!
//! Each `WindowRow` carries everything needed to restore a top-level window
//! across launches: a stable [`WindowId`], the virtual desktop GUID it was
//! last seen on, an opaque Win32 `WINDOWPLACEMENT` blob, the serialized
//! pane tree, and the wall-clock millis it was last touched.
//!
//! **Thread ownership**: only the persistence thread reads/writes this
//! table; UI threads talk to it through [`crate::PersistClient`].

use continuity_buffer::WindowId;
use crossbeam_channel::bounded;
use rusqlite::{params, Connection};
use uuid::Uuid;

use crate::message::PersistMessage;
use crate::{Error, PersistClient};

/// One row of the `windows` table. Maps 1:1 to a top-level window.
#[derive(Debug, Clone)]
pub struct WindowRow {
    /// Stable window id (UUIDv7).
    pub id: WindowId,
    /// 16-byte GUID returned by `IVirtualDesktopManager::GetWindowDesktopId`,
    /// or `None` if the desktop manager is unavailable / the window has not
    /// been queried yet.
    pub virtual_desktop_guid: Option<[u8; 16]>,
    /// Best-effort monitor identifier from `MonitorFromWindow`. `None` when
    /// not yet known.
    pub monitor_id: Option<i64>,
    /// Opaque `WINDOWPLACEMENT` blob — captured via `GetWindowPlacement` on
    /// save, replayed via `SetWindowPlacement` on restore. Never inspected
    /// by the persist layer.
    pub placement_blob: Option<Vec<u8>>,
    /// Serialized pane tree (see `crate::pane_tree_json` over in `ui`).
    pub pane_tree_json: String,
    /// Wall-clock millis the row was last touched.
    pub last_seen_ms: i64,
}

impl WindowRow {
    /// Convenience: build a row with `last_seen_ms` set to `now_ms` and no
    /// placement / desktop info yet.
    #[must_use]
    pub fn new(id: WindowId, pane_tree_json: String, now_ms: i64) -> Self {
        Self {
            id,
            virtual_desktop_guid: None,
            monitor_id: None,
            placement_blob: None,
            pane_tree_json,
            last_seen_ms: now_ms,
        }
    }
}

/// Insert or update a `windows` row. Resets `deleted_at` to `NULL`.
///
/// # Errors
///
/// Returns [`Error::Sqlite`] when the upsert fails.
pub fn save_window(conn: &Connection, row: &WindowRow) -> Result<(), Error> {
    conn.execute(
        "INSERT INTO windows (
             id, virtual_desktop_guid, monitor_id, placement_blob,
             pane_tree_json, last_seen, deleted_at
         ) VALUES (?, ?, ?, ?, ?, ?, NULL)
         ON CONFLICT(id) DO UPDATE SET
             virtual_desktop_guid = excluded.virtual_desktop_guid,
             monitor_id           = excluded.monitor_id,
             placement_blob       = excluded.placement_blob,
             pane_tree_json       = excluded.pane_tree_json,
             last_seen            = excluded.last_seen,
             deleted_at           = NULL",
        params![
            row.id.as_uuid().as_bytes().as_slice(),
            row.virtual_desktop_guid.as_ref().map(|b| b.as_slice()),
            row.monitor_id,
            row.placement_blob.as_deref(),
            row.pane_tree_json,
            row.last_seen_ms,
        ],
    )?;
    Ok(())
}

/// Soft-delete a `windows` row by stamping `deleted_at`. Subsequent
/// [`load_active_windows`] calls will skip it. Returns `true` if a row was
/// updated.
///
/// # Errors
///
/// Returns [`Error::Sqlite`] when the update fails.
pub fn delete_window(conn: &Connection, id: WindowId, now_ms: i64) -> Result<bool, Error> {
    let n = conn.execute(
        "UPDATE windows SET deleted_at = ? WHERE id = ? AND deleted_at IS NULL",
        params![now_ms, id.as_uuid().as_bytes().as_slice()],
    )?;
    Ok(n > 0)
}

/// Load every non-deleted window, most-recently-seen first.
///
/// # Errors
///
/// Returns [`Error::Sqlite`] when the query fails or
/// [`Error::Decode`] when a stored UUID blob has an unexpected size.
pub fn load_active_windows(conn: &Connection) -> Result<Vec<WindowRow>, Error> {
    let mut stmt = conn.prepare(
        "SELECT id, virtual_desktop_guid, monitor_id, placement_blob,
                pane_tree_json, last_seen
           FROM windows
          WHERE deleted_at IS NULL
          ORDER BY last_seen DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        let id_bytes: Vec<u8> = row.get(0)?;
        let guid_bytes: Option<Vec<u8>> = row.get(1)?;
        let monitor_id: Option<i64> = row.get(2)?;
        let placement_blob: Option<Vec<u8>> = row.get(3)?;
        let pane_tree_json: String = row.get(4)?;
        let last_seen_ms: i64 = row.get(5)?;
        Ok((
            id_bytes,
            guid_bytes,
            monitor_id,
            placement_blob,
            pane_tree_json,
            last_seen_ms,
        ))
    })?;
    let mut out = Vec::new();
    for r in rows {
        let (id_bytes, guid_bytes, monitor_id, placement_blob, pane_tree_json, last_seen_ms) = r?;
        let id = decode_window_id(&id_bytes)?;
        let virtual_desktop_guid = guid_bytes
            .map(|b| {
                if b.len() == 16 {
                    let mut arr = [0u8; 16];
                    arr.copy_from_slice(&b);
                    Ok(arr)
                } else {
                    Err(Error::Decode(format!(
                        "windows.virtual_desktop_guid expected 16 bytes, got {}",
                        b.len()
                    )))
                }
            })
            .transpose()?;
        out.push(WindowRow {
            id,
            virtual_desktop_guid,
            monitor_id,
            placement_blob,
            pane_tree_json,
            last_seen_ms,
        });
    }
    Ok(out)
}

fn decode_window_id(bytes: &[u8]) -> Result<WindowId, Error> {
    if bytes.len() != 16 {
        return Err(Error::Decode(format!(
            "windows.id expected 16 bytes, got {}",
            bytes.len()
        )));
    }
    let mut arr = [0u8; 16];
    arr.copy_from_slice(bytes);
    Ok(WindowId::from_uuid(Uuid::from_bytes(arr)))
}

impl PersistClient {
    /// Synchronously upsert a [`WindowRow`].
    ///
    /// # Errors
    ///
    /// Propagates any [`Error`] the persist thread reports, or
    /// [`Error::ThreadGone`] if the thread has exited.
    pub fn save_window(&self, row: WindowRow) -> Result<(), Error> {
        let (tx, rx) = bounded(1);
        self.sender()
            .send(PersistMessage::SaveWindow {
                row,
                reply: Some(tx),
            })
            .map_err(|_| Error::ThreadGone)?;
        rx.recv().map_err(|_| Error::ThreadGone)?
    }

    /// Fire-and-forget upsert of a [`WindowRow`]. Failures are logged on
    /// the persist thread.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ThreadGone`] if the persist thread has exited.
    pub fn save_window_async(&self, row: WindowRow) -> Result<(), Error> {
        self.sender()
            .send(PersistMessage::SaveWindow { row, reply: None })
            .map_err(|_| Error::ThreadGone)
    }

    /// Soft-delete a window (stamps `deleted_at`).
    ///
    /// # Errors
    ///
    /// Propagates any [`Error`] the persist thread reports, or
    /// [`Error::ThreadGone`] if the thread has exited.
    pub fn delete_window(&self, id: WindowId, now_ms: i64) -> Result<bool, Error> {
        let (tx, rx) = bounded(1);
        self.sender()
            .send(PersistMessage::DeleteWindow {
                id,
                now_ms,
                reply: tx,
            })
            .map_err(|_| Error::ThreadGone)?;
        rx.recv().map_err(|_| Error::ThreadGone)?
    }

    /// Synchronously load every non-deleted window row.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error`] the persist thread reports.
    pub fn load_active_windows(&self) -> Result<Vec<WindowRow>, Error> {
        let (tx, rx) = bounded(1);
        self.sender()
            .send(PersistMessage::LoadActiveWindows { reply: tx })
            .map_err(|_| Error::ThreadGone)?;
        rx.recv().map_err(|_| Error::ThreadGone)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn save_and_load_round_trips() {
        let store = Store::open_in_memory().unwrap();
        let id = WindowId::new();
        let row = WindowRow {
            id,
            virtual_desktop_guid: Some([7u8; 16]),
            monitor_id: Some(1),
            placement_blob: Some(vec![1, 2, 3, 4]),
            pane_tree_json: "{\"k\":1}".to_string(),
            last_seen_ms: 12_345,
        };
        save_window(store.conn(), &row).unwrap();
        let rows = load_active_windows(store.conn()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].virtual_desktop_guid, Some([7u8; 16]));
        assert_eq!(rows[0].monitor_id, Some(1));
        assert_eq!(rows[0].placement_blob.as_deref(), Some(&[1, 2, 3, 4][..]));
        assert_eq!(rows[0].pane_tree_json, "{\"k\":1}");
        assert_eq!(rows[0].last_seen_ms, 12_345);
    }

    #[test]
    fn save_is_upsert() {
        let store = Store::open_in_memory().unwrap();
        let id = WindowId::new();
        let mut row = WindowRow::new(id, "{}".into(), 100);
        save_window(store.conn(), &row).unwrap();
        row.last_seen_ms = 200;
        row.pane_tree_json = "{\"v\":2}".into();
        save_window(store.conn(), &row).unwrap();
        let rows = load_active_windows(store.conn()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].last_seen_ms, 200);
        assert_eq!(rows[0].pane_tree_json, "{\"v\":2}");
    }

    #[test]
    fn delete_hides_from_active_load() {
        let store = Store::open_in_memory().unwrap();
        let id = WindowId::new();
        let row = WindowRow::new(id, "{}".into(), 100);
        save_window(store.conn(), &row).unwrap();
        let n = delete_window(store.conn(), id, 200).unwrap();
        assert!(n);
        let rows = load_active_windows(store.conn()).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn save_after_delete_revives_row() {
        let store = Store::open_in_memory().unwrap();
        let id = WindowId::new();
        let row = WindowRow::new(id, "{}".into(), 100);
        save_window(store.conn(), &row).unwrap();
        delete_window(store.conn(), id, 200).unwrap();
        save_window(store.conn(), &row).unwrap();
        let rows = load_active_windows(store.conn()).unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn load_orders_most_recent_first() {
        let store = Store::open_in_memory().unwrap();
        let a = WindowRow::new(WindowId::new(), "{}".into(), 100);
        let b = WindowRow::new(WindowId::new(), "{}".into(), 300);
        let c = WindowRow::new(WindowId::new(), "{}".into(), 200);
        save_window(store.conn(), &a).unwrap();
        save_window(store.conn(), &b).unwrap();
        save_window(store.conn(), &c).unwrap();
        let rows = load_active_windows(store.conn()).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].id, b.id);
        assert_eq!(rows[1].id, c.id);
        assert_eq!(rows[2].id, a.id);
    }
}
