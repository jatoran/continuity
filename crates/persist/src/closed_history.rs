//! Closed-history stack — schema v5.
//!
//! A single ordered stack of close events spanning every window. Powers
//! the smart `tab.reopen_closed` (Ctrl+Shift+T) handler: closing a whole
//! window pushes a [`ClosedHistoryKind::Window`] entry carrying the
//! pane-tree JSON snapshot at close time, so the next reopen request
//! can reconstruct the entire window — including all its tabs, splits,
//! and placement — even after the source window's row has been
//! tombstoned in the `windows` table.
//!
//! The stack is bounded to [`STACK_CAP`] entries; the oldest entries
//! roll off as new closes arrive, mirroring the per-window
//! `PaneTree::recently_closed` cap.
//!
//! **Thread ownership**: only the persistence thread reads/writes this
//! table; UI / registry threads talk to it through [`PersistClient`].

use continuity_buffer::WindowId;
use crossbeam_channel::bounded;
use rusqlite::{params, Connection};
use uuid::Uuid;

use crate::message::PersistMessage;
use crate::{Error, PersistClient};

/// Maximum entries retained in the closed-history stack. Older entries
/// are evicted as new closes arrive.
pub const STACK_CAP: usize = 32;

/// Kind of unit recorded by a [`ClosedHistoryEntry`]. Today only
/// `Window` is produced — `Tab` and `Pane` variants are reserved for
/// future per-pane / per-tab cross-window reopen plumbing (see report
/// § 7 follow-ups). Stored as TEXT in SQLite to keep the schema
/// migration-free if new kinds are added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClosedHistoryKind {
    /// A whole window was closed; payload is its pane-tree JSON.
    Window,
    /// A single tab was closed in a window that may or may not be
    /// alive. Reserved.
    Tab,
    /// A pane (split group) was closed. Reserved.
    Pane,
}

impl ClosedHistoryKind {
    /// SQLite encoding.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ClosedHistoryKind::Window => "window",
            ClosedHistoryKind::Tab => "tab",
            ClosedHistoryKind::Pane => "pane",
        }
    }

    /// Parse the SQLite encoding back into a typed kind. Unknown
    /// strings — e.g. from a future schema that introduced new kinds
    /// — return `None`; the caller treats them as inert (skip and
    /// pop the next entry) rather than panicking.
    #[must_use]
    pub fn parse_sql(s: &str) -> Option<Self> {
        match s {
            "window" => Some(ClosedHistoryKind::Window),
            "tab" => Some(ClosedHistoryKind::Tab),
            "pane" => Some(ClosedHistoryKind::Pane),
            _ => None,
        }
    }
}

/// One row of the `closed_history` table.
#[derive(Debug, Clone)]
pub struct ClosedHistoryEntry {
    /// SQLite rowid — used by [`pop_closed_history`] to delete this
    /// specific row after the caller has reconstructed the unit.
    pub id: i64,
    /// Wall-clock millis at which the unit was closed.
    pub closed_at_ms: i64,
    /// What kind of unit was closed.
    pub kind: ClosedHistoryKind,
    /// Source window id, when known. `Window` entries carry the closed
    /// window's id so smart-reopen can dedupe against any concurrently
    /// resurrected row in the `windows` table.
    pub window_id: Option<WindowId>,
    /// JSON payload — for `Window` entries this is the pane-tree JSON
    /// produced by [`crate::WindowRow::pane_tree_json`].
    pub payload_json: String,
}

/// Push one entry onto the closed-history stack, then evict the oldest
/// entries beyond [`STACK_CAP`].
///
/// # Errors
///
/// Returns [`Error::Sqlite`] when either the insert or the eviction
/// statement fails.
pub fn push_closed_history(
    conn: &Connection,
    kind: ClosedHistoryKind,
    window_id: Option<WindowId>,
    payload_json: &str,
    closed_at_ms: i64,
) -> Result<(), Error> {
    conn.execute(
        "INSERT INTO closed_history (closed_at_ms, kind, window_id, payload_json)
         VALUES (?, ?, ?, ?)",
        params![
            closed_at_ms,
            kind.as_str(),
            window_id.map(|w| w.as_uuid().as_bytes().to_vec()),
            payload_json,
        ],
    )?;
    // Evict any entries beyond the cap. Bounded delete by id keeps the
    // table small and the index hot.
    conn.execute(
        "DELETE FROM closed_history
          WHERE id NOT IN (
              SELECT id FROM closed_history ORDER BY id DESC LIMIT ?
          )",
        params![STACK_CAP as i64],
    )?;
    if continuity_trace::is_enabled() {
        continuity_trace::log_event(
            "closed_history_push",
            &format!(
                "kind={} window_id={} payload_bytes={} closed_at_ms={}",
                kind.as_str(),
                window_id
                    .map(|w| w.as_uuid().to_string())
                    .unwrap_or_else(|| "none".to_string()),
                payload_json.len(),
                closed_at_ms,
            ),
        );
    }
    Ok(())
}

/// Peek the newest entry without modifying the stack. Returns `None`
/// when the stack is empty.
///
/// # Errors
///
/// Returns [`Error::Sqlite`] when the query fails or [`Error::Decode`]
/// when a stored window id has an unexpected size.
pub fn peek_closed_history(conn: &Connection) -> Result<Option<ClosedHistoryEntry>, Error> {
    let mut stmt = conn.prepare(
        "SELECT id, closed_at_ms, kind, window_id, payload_json
           FROM closed_history
          ORDER BY id DESC
          LIMIT 1",
    )?;
    let mut rows = stmt.query([])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let id: i64 = row.get(0)?;
    let closed_at_ms: i64 = row.get(1)?;
    let kind_str: String = row.get(2)?;
    let window_id_blob: Option<Vec<u8>> = row.get(3)?;
    let payload_json: String = row.get(4)?;
    let Some(kind) = ClosedHistoryKind::parse_sql(&kind_str) else {
        // Unknown kind from a future schema — surface nothing rather
        // than crash. The caller's "no entry" branch is the safe
        // fallback (local recently_closed continues to work).
        return Ok(None);
    };
    let window_id = window_id_blob
        .map(|b| {
            if b.len() == 16 {
                let mut arr = [0u8; 16];
                arr.copy_from_slice(&b);
                Ok(WindowId::from_uuid(Uuid::from_bytes(arr)))
            } else {
                Err(Error::Decode(format!(
                    "closed_history.window_id expected 16 bytes, got {}",
                    b.len()
                )))
            }
        })
        .transpose()?;
    Ok(Some(ClosedHistoryEntry {
        id,
        closed_at_ms,
        kind,
        window_id,
        payload_json,
    }))
}

/// Pop the newest entry — equivalent to [`peek_closed_history`]
/// followed by a `DELETE` against the same row. Returns the popped
/// entry, or `None` when the stack was empty.
///
/// # Errors
///
/// Returns [`Error::Sqlite`] when the delete fails or
/// [`Error::Decode`] when the stored window id has an unexpected size.
pub fn pop_closed_history(conn: &Connection) -> Result<Option<ClosedHistoryEntry>, Error> {
    let Some(entry) = peek_closed_history(conn)? else {
        if continuity_trace::is_enabled() {
            continuity_trace::log_event("closed_history_pop", "outcome=empty");
        }
        return Ok(None);
    };
    conn.execute("DELETE FROM closed_history WHERE id = ?", params![entry.id])?;
    if continuity_trace::is_enabled() {
        continuity_trace::log_event(
            "closed_history_pop",
            &format!(
                "outcome=ok kind={} window_id={} payload_bytes={} closed_at_ms={}",
                entry.kind.as_str(),
                entry
                    .window_id
                    .map(|w| w.as_uuid().to_string())
                    .unwrap_or_else(|| "none".to_string()),
                entry.payload_json.len(),
                entry.closed_at_ms,
            ),
        );
    }
    Ok(Some(entry))
}

impl PersistClient {
    /// Synchronously push one entry onto the closed-history stack.
    /// Blocks until the persist thread acks.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error`] the persist thread reports, or
    /// [`Error::ThreadGone`] if the thread has exited.
    pub fn push_closed_history(
        &self,
        kind: ClosedHistoryKind,
        window_id: Option<WindowId>,
        payload_json: String,
        closed_at_ms: i64,
    ) -> Result<(), Error> {
        let (tx, rx) = bounded(1);
        self.sender()
            .send(PersistMessage::PushClosedHistory {
                kind,
                window_id,
                payload_json,
                closed_at_ms,
                reply: tx,
            })
            .map_err(|_| Error::ThreadGone)?;
        rx.recv().map_err(|_| Error::ThreadGone)?
    }

    /// Pop the newest closed-history entry, if any.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error`] the persist thread reports.
    pub fn pop_closed_history(&self) -> Result<Option<ClosedHistoryEntry>, Error> {
        let (tx, rx) = bounded(1);
        self.sender()
            .send(PersistMessage::PopClosedHistory { reply: tx })
            .map_err(|_| Error::ThreadGone)?;
        rx.recv().map_err(|_| Error::ThreadGone)?
    }

    /// Peek the newest closed-history entry, without modifying the stack.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error`] the persist thread reports.
    pub fn peek_closed_history(&self) -> Result<Option<ClosedHistoryEntry>, Error> {
        let (tx, rx) = bounded(1);
        self.sender()
            .send(PersistMessage::PeekClosedHistory { reply: tx })
            .map_err(|_| Error::ThreadGone)?;
        rx.recv().map_err(|_| Error::ThreadGone)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn push_and_peek_round_trips() {
        let store = Store::open_in_memory().unwrap();
        let win = WindowId::new();
        push_closed_history(
            store.conn(),
            ClosedHistoryKind::Window,
            Some(win),
            "{\"k\":1}",
            12_345,
        )
        .unwrap();
        let entry = peek_closed_history(store.conn()).unwrap().unwrap();
        assert_eq!(entry.kind, ClosedHistoryKind::Window);
        assert_eq!(entry.closed_at_ms, 12_345);
        assert_eq!(entry.window_id, Some(win));
        assert_eq!(entry.payload_json, "{\"k\":1}");
    }

    #[test]
    fn pop_is_destructive() {
        let store = Store::open_in_memory().unwrap();
        push_closed_history(store.conn(), ClosedHistoryKind::Window, None, "{}", 1).unwrap();
        assert!(pop_closed_history(store.conn()).unwrap().is_some());
        assert!(peek_closed_history(store.conn()).unwrap().is_none());
    }

    #[test]
    fn newest_pops_first() {
        let store = Store::open_in_memory().unwrap();
        for i in 0..3 {
            push_closed_history(
                store.conn(),
                ClosedHistoryKind::Window,
                None,
                &format!("{{\"k\":{i}}}"),
                100 + i,
            )
            .unwrap();
        }
        let e = pop_closed_history(store.conn()).unwrap().unwrap();
        assert_eq!(e.payload_json, "{\"k\":2}");
        let e = pop_closed_history(store.conn()).unwrap().unwrap();
        assert_eq!(e.payload_json, "{\"k\":1}");
    }

    #[test]
    fn stack_evicts_beyond_cap() {
        let store = Store::open_in_memory().unwrap();
        for i in 0..(STACK_CAP + 5) {
            push_closed_history(
                store.conn(),
                ClosedHistoryKind::Window,
                None,
                &format!("{{\"k\":{i}}}"),
                i as i64,
            )
            .unwrap();
        }
        let count: i64 = store
            .conn()
            .query_row("SELECT count(*) FROM closed_history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, STACK_CAP as i64);
        // Newest entry still sits on top.
        let e = peek_closed_history(store.conn()).unwrap().unwrap();
        let expected = format!("{{\"k\":{}}}", STACK_CAP + 4);
        assert_eq!(e.payload_json, expected);
    }

    #[test]
    fn unknown_kind_string_returns_none() {
        let store = Store::open_in_memory().unwrap();
        store
            .conn()
            .execute(
                "INSERT INTO closed_history (closed_at_ms, kind, window_id, payload_json)
                 VALUES (?, ?, NULL, ?)",
                params![1i64, "future_kind", "{}"],
            )
            .unwrap();
        assert!(peek_closed_history(store.conn()).unwrap().is_none());
    }
}
