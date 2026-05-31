//! δ.4 — buffer-record listing for the previous-buffer browser overlay.
//!
//! Exposes `Store::list_buffer_records(filter)` returning one
//! [`BufferRecord`] per row in `buffers`, optionally restricted to the
//! non-trashed / trashed-only subset. The persist thread also decodes
//! the latest snapshot for each row (if present) so the UI can render
//! a derived title without a second round-trip per buffer.
//!
//! Thread ownership: every function takes `&Store`, whose connection
//! is owned by the persistence thread. Invoked from the persist loop
//! in response to [`crate::PersistMessage::ListBufferRecords`].

use continuity_buffer::BufferId;
use uuid::Uuid;

use crate::checksum::fnv1a_64;
use crate::store::Store;
use crate::Error;

/// Filter for [`Store::list_buffer_records`].
///
/// Default is [`Self::ActiveOnly`] — the headline previous-buffer
/// browser shows non-trashed buffers. The other variants let the user
/// pivot via an in-overlay chord without re-opening.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum BufferListFilter {
    /// Only buffers with `deleted_at IS NULL` (i.e. not in trash).
    #[default]
    ActiveOnly,
    /// Every buffer row, including trashed.
    All,
    /// Only buffers whose `deleted_at` is set.
    TrashedOnly,
}

/// One row produced by [`Store::list_buffer_records`].
///
/// `title` is best-effort: derived from the first non-empty trimmed
/// line of the latest snapshot's decoded content, or `None` when the
/// buffer has no snapshot, when the snapshot fails to decode, or when
/// the content is pure-whitespace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferRecord {
    /// Buffer id.
    pub id: BufferId,
    /// Derived title (first non-empty trimmed line of the latest
    /// snapshot content), or `None`.
    pub title: Option<String>,
    /// Buffer creation time (unix ms).
    pub created_at_ms: i64,
    /// Last activity timestamp (unix ms).
    pub last_touched_ms: i64,
    /// Number of rows in `buffer_edits` for this buffer.
    pub edit_count: u64,
    /// `true` when the row carries a non-NULL `deleted_at`.
    pub is_trashed: bool,
}

impl Store {
    /// δ.4: enumerate every buffer row matching `filter`, sorted by
    /// `last_touched DESC`. The latest snapshot blob (if present) is
    /// decoded inline so the caller can render a title without a
    /// second per-row trip across the channel.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error::Sqlite`] from the query, or the codec
    /// errors raised by [`crate::checksum::fnv1a_64`] mismatches and
    /// zstd decode failures (logged + skipped per row; one bad
    /// snapshot does not abort the whole listing).
    pub fn list_buffer_records(
        &self,
        filter: BufferListFilter,
    ) -> Result<Vec<BufferRecord>, Error> {
        // The deleted_at clause partitions active/trashed; the
        // non-empty clause hides buffers with zero content — no
        // edits, no snapshots, and no file association. Those rows
        // accumulate from `tab.new` (fresh empty buffer) sessions
        // that the user closes without typing; surfacing them in the
        // history view is pure clutter. A buffer with a file
        // association but no edits is the "opened a blank file"
        // case and is kept — the file path itself is the identity.
        let deleted_clause = match filter {
            BufferListFilter::ActiveOnly => "b.deleted_at IS NULL",
            BufferListFilter::TrashedOnly => "b.deleted_at IS NOT NULL",
            BufferListFilter::All => "1=1",
        };
        // Empty-snapshot rows (byte_len = 0) are the baseline
        // snapshot the core thread writes for every freshly-adopted
        // buffer; they don't represent actual user content, so a
        // buffer whose ONLY snapshot is zero-byte is still treated
        // as an orphan and filtered out. A non-zero snapshot OR any
        // edit row OR a file association = real content, keep.
        let non_empty_clause = "(
            EXISTS (SELECT 1 FROM buffer_edits e WHERE e.buffer_id = b.id)
            OR EXISTS (
                SELECT 1 FROM buffer_snapshots s2
                 WHERE s2.buffer_id = b.id AND s2.byte_len > 0
            )
            OR b.file_path IS NOT NULL
        )";
        let sql = format!(
            "SELECT b.id,
                    b.created_at,
                    b.last_touched,
                    b.deleted_at,
                    (SELECT COUNT(*) FROM buffer_edits e WHERE e.buffer_id = b.id) AS edits,
                    s.content_blob,
                    s.checksum,
                    s.revision
             FROM buffers b
             LEFT JOIN buffer_snapshots s
               ON s.buffer_id = b.id
              AND s.revision = (
                    SELECT MAX(revision)
                      FROM buffer_snapshots
                     WHERE buffer_id = b.id
              )
             WHERE {deleted_clause} AND {non_empty_clause}
             ORDER BY b.last_touched DESC"
        );
        let conn = self.conn();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| {
            let id_bytes: Vec<u8> = r.get(0)?;
            let created_at_ms: i64 = r.get(1)?;
            let last_touched_ms: i64 = r.get(2)?;
            let deleted_at: Option<i64> = r.get(3)?;
            let edit_count: i64 = r.get(4)?;
            let blob: Option<Vec<u8>> = r.get(5)?;
            let checksum: Option<i64> = r.get(6)?;
            let _revision: Option<i64> = r.get(7)?;
            Ok((
                id_bytes,
                created_at_ms,
                last_touched_ms,
                deleted_at,
                edit_count,
                blob,
                checksum,
            ))
        })?;

        let mut out: Vec<BufferRecord> = Vec::new();
        for row in rows {
            let (id_bytes, created_at_ms, last_touched_ms, deleted_at, edit_count, blob, checksum) =
                row?;
            let Some(id) = decode_buffer_id(&id_bytes) else {
                continue;
            };
            let title = decode_title(blob.as_deref(), checksum);
            out.push(BufferRecord {
                id,
                title,
                created_at_ms,
                last_touched_ms,
                edit_count: edit_count.max(0) as u64,
                is_trashed: deleted_at.is_some(),
            });
        }
        Ok(out)
    }
}

fn decode_buffer_id(bytes: &[u8]) -> Option<BufferId> {
    <[u8; 16]>::try_from(bytes)
        .ok()
        .map(|b| BufferId::from_uuid(Uuid::from_bytes(b)))
}

/// Decode the snapshot blob and extract the first non-empty trimmed
/// line as a title. Returns `None` for missing blobs, decode errors,
/// checksum mismatch, or pure-whitespace content.
fn decode_title(blob: Option<&[u8]>, checksum: Option<i64>) -> Option<String> {
    let blob = blob?;
    let bytes = zstd::stream::decode_all(blob).ok()?;
    if let Some(expected) = checksum {
        if fnv1a_64(&bytes) != expected as u64 {
            return None;
        }
    }
    let text = std::str::from_utf8(&bytes).ok()?;
    first_non_empty_trimmed_line(text)
}

/// Max characters for titles surfaced by persisted buffer pickers.
pub(crate) const BUFFER_RECORD_TITLE_MAX_CHARS: usize = 48;

/// Return the first non-empty trimmed line clipped to a sensible
/// display width, or `None` when no such line exists.
pub(crate) fn first_non_empty_trimmed_line(s: &str) -> Option<String> {
    for line in s.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        return Some(clip_with_ellipsis(
            strip_heading_prefix(trimmed),
            BUFFER_RECORD_TITLE_MAX_CHARS,
        ));
    }
    None
}

fn strip_heading_prefix(line: &str) -> &str {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if hashes == 0 {
        return line;
    }
    let after_hashes = &line[hashes..];
    let after = after_hashes.trim_start_matches(' ');
    if after.is_empty() {
        return line;
    }
    after
}

pub(crate) fn clip_with_ellipsis(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    if max_chars == 0 {
        return String::new();
    }
    let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_buffer::Buffer;

    #[test]
    fn first_non_empty_trimmed_line_skips_blank_lines() {
        let s = "\n\n  \nhello world\nignored\n";
        assert_eq!(
            first_non_empty_trimmed_line(s),
            Some("hello world".to_string())
        );
    }

    #[test]
    fn first_non_empty_trimmed_line_pure_whitespace_is_none() {
        assert!(first_non_empty_trimmed_line("   \n\t\n  ").is_none());
    }

    #[test]
    fn first_non_empty_trimmed_line_clips_at_char_boundary() {
        let s = "é".repeat(200);
        let got = first_non_empty_trimmed_line(&s).unwrap();
        assert_eq!(got.chars().count(), BUFFER_RECORD_TITLE_MAX_CHARS);
        assert!(got.ends_with('…'));
    }

    #[test]
    fn first_non_empty_trimmed_line_strips_heading_marker() {
        assert_eq!(
            first_non_empty_trimmed_line("## Heading"),
            Some("Heading".to_string())
        );
    }

    #[test]
    fn list_buffer_records_empty_db() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.db")).unwrap();
        let rows = store
            .list_buffer_records(BufferListFilter::ActiveOnly)
            .unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn list_buffer_records_returns_title_from_latest_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.db")).unwrap();
        let buf = Buffer::from_text("# Heading\nbody\n");
        store.save_snapshot(buf.id(), &buf.snapshot()).unwrap();

        let rows = store
            .list_buffer_records(BufferListFilter::ActiveOnly)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, buf.id());
        assert_eq!(rows[0].title.as_deref(), Some("Heading"));
        assert!(!rows[0].is_trashed);
    }

    #[test]
    fn list_buffer_records_sorts_by_last_touched_desc() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.db")).unwrap();
        let b1 = Buffer::from_text("one");
        let b2 = Buffer::from_text("two");
        let b3 = Buffer::from_text("three");
        store.save_snapshot(b1.id(), &b1.snapshot()).unwrap();
        store.save_snapshot(b2.id(), &b2.snapshot()).unwrap();
        store.save_snapshot(b3.id(), &b3.snapshot()).unwrap();
        // Bump b2 to the front.
        store.touch_buffer(b2.id(), i64::MAX).unwrap();
        let rows = store
            .list_buffer_records(BufferListFilter::ActiveOnly)
            .unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].id, b2.id());
    }

    #[test]
    fn list_buffer_records_excludes_trashed_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.db")).unwrap();
        let b1 = Buffer::from_text("kept");
        let b2 = Buffer::from_text("deleted");
        store.save_snapshot(b1.id(), &b1.snapshot()).unwrap();
        store.save_snapshot(b2.id(), &b2.snapshot()).unwrap();
        store.move_to_trash(b2.id(), 1_000, 7).unwrap();

        let active = store
            .list_buffer_records(BufferListFilter::ActiveOnly)
            .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, b1.id());

        let trashed = store
            .list_buffer_records(BufferListFilter::TrashedOnly)
            .unwrap();
        assert_eq!(trashed.len(), 1);
        assert_eq!(trashed[0].id, b2.id());
        assert!(trashed[0].is_trashed);

        let all = store.list_buffer_records(BufferListFilter::All).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn list_buffer_records_edit_count_starts_at_zero() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.db")).unwrap();
        let buf = Buffer::from_text("hi");
        store.save_snapshot(buf.id(), &buf.snapshot()).unwrap();
        let rows = store
            .list_buffer_records(BufferListFilter::ActiveOnly)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].edit_count, 0);
    }
}
