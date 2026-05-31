//! FTS5-backed quick-open / palette index over buffer titles + content.

use std::path::Path;

use continuity_buffer::BufferId;
use rusqlite::{params, Connection};

use crate::Error;

/// One result row from an FTS5 query.
#[derive(Debug, Clone)]
pub struct SearchHit {
    /// Matching buffer.
    pub buffer_id: BufferId,
    /// Matching buffer's title.
    pub title: String,
    /// FTS5 `snippet()` excerpt highlighting the match.
    pub snippet: String,
    /// `bm25` rank (lower is more relevant).
    pub rank: f64,
}

/// FTS5 quick-open index.
pub struct SearchIndex {
    conn: Connection,
}

const SCHEMA: &str = "
CREATE VIRTUAL TABLE IF NOT EXISTS fts_buffers USING fts5(
    buffer_id UNINDEXED,
    title,
    content,
    tokenize = 'unicode61'
);
";

impl SearchIndex {
    /// Open a persistent index at `path`.
    pub fn open(path: &Path) -> Result<Self, Error> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// In-memory index for tests.
    pub fn open_in_memory() -> Result<Self, Error> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// Insert or replace a buffer's row.
    pub fn upsert(&self, id: BufferId, title: &str, content: &str) -> Result<(), Error> {
        let id_str = id.as_uuid().to_string();
        // FTS5 doesn't support ON CONFLICT; do delete + insert.
        self.conn.execute(
            "DELETE FROM fts_buffers WHERE buffer_id = ?1",
            params![id_str],
        )?;
        self.conn.execute(
            "INSERT INTO fts_buffers(buffer_id, title, content) VALUES (?1, ?2, ?3)",
            params![id_str, title, content],
        )?;
        Ok(())
    }

    /// Remove a buffer.
    pub fn delete(&self, id: BufferId) -> Result<(), Error> {
        let id_str = id.as_uuid().to_string();
        self.conn.execute(
            "DELETE FROM fts_buffers WHERE buffer_id = ?1",
            params![id_str],
        )?;
        Ok(())
    }

    /// Run an FTS5 query, returning up to `limit` hits ranked by `bm25`.
    pub fn query(&self, q: &str, limit: u32) -> Result<Vec<SearchHit>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT buffer_id, title, snippet(fts_buffers, 2, '<', '>', '...', 8), rank
             FROM fts_buffers
             WHERE fts_buffers MATCH ?1
             ORDER BY rank
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![q, limit as i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, f64>(3)?,
            ))
        })?;
        let mut hits = Vec::new();
        for row in rows {
            let (id_str, title, snippet, rank) = row?;
            let uuid: uuid::Uuid =
                uuid::Uuid::parse_str(&id_str).map_err(|e| Error::InvalidId(e.to_string()))?;
            hits.push(SearchHit {
                buffer_id: BufferId::from_uuid(uuid),
                title,
                snippet,
                rank,
            });
        }
        Ok(hits)
    }

    /// Number of indexed buffers.
    pub fn count(&self) -> Result<u64, Error> {
        let n: i64 = self
            .conn
            .query_row("SELECT count(*) FROM fts_buffers", [], |r| r.get(0))?;
        Ok(n as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_index_count_zero() {
        let idx = SearchIndex::open_in_memory().unwrap();
        assert_eq!(idx.count().unwrap(), 0);
    }

    #[test]
    fn upsert_and_query_finds_match() {
        let idx = SearchIndex::open_in_memory().unwrap();
        let id = BufferId::new();
        idx.upsert(id, "Notes on Rust", "ropey is a rope library")
            .unwrap();
        idx.upsert(BufferId::new(), "Recipes", "garlic soup")
            .unwrap();

        let hits = idx.query("rope", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Notes on Rust");
        assert_eq!(hits[0].buffer_id, id);
        assert!(hits[0].snippet.contains('<'));
    }

    #[test]
    fn upsert_replaces_existing() {
        let idx = SearchIndex::open_in_memory().unwrap();
        let id = BufferId::new();
        idx.upsert(id, "v1", "first").unwrap();
        idx.upsert(id, "v2", "second").unwrap();
        assert_eq!(idx.count().unwrap(), 1);
        let hits = idx.query("second", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "v2");
    }

    #[test]
    fn delete_removes_row() {
        let idx = SearchIndex::open_in_memory().unwrap();
        let id = BufferId::new();
        idx.upsert(id, "t", "c").unwrap();
        idx.delete(id).unwrap();
        assert_eq!(idx.count().unwrap(), 0);
    }

    #[test]
    fn query_returns_empty_for_no_matches() {
        let idx = SearchIndex::open_in_memory().unwrap();
        idx.upsert(BufferId::new(), "title", "content").unwrap();
        let hits = idx.query("absentword", 10).unwrap();
        assert!(hits.is_empty());
    }
}
