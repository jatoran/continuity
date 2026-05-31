//! Buffer-history timeline persistence helpers.
//!
//! Powers the buffer-history tab (a swimlane visualization of every
//! buffer that lives in the DB, with one row per buffer and a horizontal
//! time axis stamped with snapshot dots). Produces one
//! [`BufferHistoryLane`] per matching [`crate::buffer_listing::BufferRecord`]
//! with the snapshot timestamps already grouped by buffer, so the UI can
//! render the entire chart from a single round-trip across the persist
//! channel.
//!
//! Thread ownership: every function takes `&Store`, whose connection is
//! single-threaded (the persistence thread). Invoked from the persist
//! loop in response to [`crate::PersistMessage::ListBufferHistoryTimeline`].

use std::collections::HashMap;

use continuity_buffer::BufferId;
use uuid::Uuid;

use crate::buffer_listing::{
    clip_with_ellipsis, first_non_empty_trimmed_line, BufferListFilter, BufferRecord,
};
use crate::store::Store;
use crate::Error;

/// Derive a `(title, preview)` pair from the materialized current
/// content of a buffer. Both come from the same source so the
/// history tab's title row, hover preview band, and click-to-open
/// outcome are forced consistent. `None` for both when the content
/// is missing or empty — the UI surfaces that as `"Untitled"` +
/// `"(no persisted content preview)"`.
fn derive_title_and_preview(content: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(text) = content else {
        return (None, None);
    };
    if text.trim().is_empty() {
        return (None, None);
    }
    let title = first_non_empty_trimmed_line(text);
    let preview = derive_preview(text);
    (title, preview)
}

fn derive_preview(text: &str) -> Option<String> {
    let mut lines = Vec::new();
    for line in text.lines().take(PREVIEW_MAX_LINES) {
        let trimmed = line.trim_end_matches('\r');
        lines.push(clip_with_ellipsis(trimmed, PREVIEW_LINE_MAX_CHARS));
    }
    if lines.is_empty() {
        None
    } else {
        let preview = lines.join("\n");
        Some(clip_with_ellipsis(&preview, PREVIEW_MAX_CHARS))
    }
}

fn derive_content_counts(content: Option<&str>) -> (usize, usize) {
    let Some(text) = content else {
        return (0, 0);
    };
    let line_count = text
        .as_bytes()
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        .saturating_add(1);
    let char_count = text.chars().count();
    (line_count, char_count)
}

/// One swimlane in the buffer-history visualization.
///
/// Pairs a [`BufferRecord`] (id, derived title, creation / last-touched
/// timestamps, edit count, trashed flag) with the ascending list of
/// snapshot timestamps that the renderer projects as tick marks along
/// the lane's time axis, current content counts for the row subtitle,
/// plus an optional content preview rendered on hover/selection so the
/// user can confirm which buffer they're about to reopen without
/// leaving the chart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferHistoryLane {
    /// The underlying buffer record (carries id, title, age, edit count,
    /// trashed flag — see [`BufferRecord`]).
    pub record: BufferRecord,
    /// Snapshot timestamps for this buffer, ascending. Empty when the
    /// buffer has been persisted (touched) but never snapshotted.
    pub snapshot_times_ms: Vec<i64>,
    /// Total logical source lines in the latest materialized content.
    /// Uses the same newline-count-plus-one convention as `Rope::len_lines`;
    /// `0` means content could not be materialized.
    pub line_count: usize,
    /// Total Unicode scalar values in the latest materialized content.
    pub char_count: usize,
    /// First [`PREVIEW_MAX_CHARS`] characters (UTF-8 boundary-clipped)
    /// of the buffer's latest snapshot content, or `None` when the
    /// snapshot is missing / undecodable. Used by the hover preview
    /// band so the user can read a few lines of the buffer before
    /// reopening it.
    pub preview: Option<String>,
}

/// Max characters returned in [`BufferHistoryLane::preview`]. Sized to
/// fit roughly the first six 60-column lines of a typical markdown
/// note, which is enough to confirm a buffer's identity at hover
/// without ballooning the per-buffer payload.
pub const PREVIEW_MAX_CHARS: usize = 360;
/// Max number of source lines shown in a history preview band.
pub const PREVIEW_MAX_LINES: usize = 6;
/// Max characters per source line before adding an ellipsis in previews.
pub const PREVIEW_LINE_MAX_CHARS: usize = 96;

impl Store {
    /// Enumerate every buffer matching `filter` and pair each with the
    /// ascending list of snapshot timestamps under it.
    ///
    /// The buffer summary is the same shape produced by
    /// [`Store::list_buffer_records`]; the snapshot timestamps come from
    /// a single bulk `SELECT buffer_id, created_at FROM buffer_snapshots`
    /// grouped in Rust. Two queries total, regardless of how many
    /// buffers are in the result set — the UI never pays the N+1
    /// snapshot lookup cost.
    ///
    /// Ordering: lanes are returned in `last_touched DESC` (so the
    /// most-recently-active buffer is lane 0); within a lane, snapshot
    /// timestamps are ascending so the renderer can scan them as a
    /// sorted sequence.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error::Sqlite`] from either underlying query.
    pub fn load_buffer_history_timeline(
        &self,
        filter: BufferListFilter,
    ) -> Result<Vec<BufferHistoryLane>, Error> {
        let records = self.list_buffer_records(filter)?;
        if records.is_empty() {
            return Ok(Vec::new());
        }
        let snapshot_map = self.snapshot_timestamps_grouped(filter)?;
        let lanes = records
            .into_iter()
            .map(|mut record| {
                let snapshot_times_ms = snapshot_map.get(&record.id).cloned().unwrap_or_default();
                // Materialize the **current** rope state via the
                // same snapshot-plus-edit-replay path the recovery
                // flow uses. Both `title` and `preview` are derived
                // from this single source of truth so the row's
                // displayed title, the hover preview band, and the
                // buffer that opens on click can't diverge — any
                // mismatch was the entire reason the previous code
                // surfaced "Hello" titles with blank-opening tabs.
                // Sentinel revision = `i64::MAX` interpreted as `u64`
                // (rather than `u64::MAX`) because the underlying
                // SQL stores revisions as `INTEGER` and binds them
                // via `revision.get() as i64`. `u64::MAX as i64`
                // wraps to `-1`, which makes the
                // `WHERE revision <= ?` predicate match zero rows
                // and silently returns `None` for every buffer.
                let materialized = self
                    .load_content_at_revision(
                        record.id,
                        continuity_buffer::Revision(i64::MAX as u64),
                    )
                    .ok()
                    .flatten();
                let (line_count, char_count) = derive_content_counts(materialized.as_deref());
                let (title, preview) = derive_title_and_preview(materialized.as_deref());
                record.title = title;
                BufferHistoryLane {
                    record,
                    snapshot_times_ms,
                    line_count,
                    char_count,
                    preview,
                }
            })
            .collect();
        Ok(lanes)
    }

    /// Group every snapshot's `created_at` by buffer id. The
    /// returned vectors are ascending in time. Buffers absent from the
    /// `buffer_snapshots` table simply do not appear in the map (the
    /// caller defaults to an empty `Vec`).
    fn snapshot_timestamps_grouped(
        &self,
        filter: BufferListFilter,
    ) -> Result<HashMap<BufferId, Vec<i64>>, Error> {
        // Filter at the SQL boundary: the trashed-only / active-only
        // discriminant should not paint dots for buffers that are about
        // to be excluded from the lane list. The join against `buffers`
        // is cheap (PK lookup) and keeps the renderer honest.
        let where_clause = match filter {
            BufferListFilter::ActiveOnly => "WHERE b.deleted_at IS NULL",
            BufferListFilter::TrashedOnly => "WHERE b.deleted_at IS NOT NULL",
            BufferListFilter::All => "",
        };
        let sql = format!(
            "SELECT s.buffer_id, s.created_at
             FROM buffer_snapshots s
             JOIN buffers b ON b.id = s.buffer_id
             {where_clause}
             ORDER BY s.buffer_id ASC, s.created_at ASC"
        );
        let conn = self.conn();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| {
            let id_bytes: Vec<u8> = r.get(0)?;
            let created_at_ms: i64 = r.get(1)?;
            Ok((id_bytes, created_at_ms))
        })?;
        let mut out: HashMap<BufferId, Vec<i64>> = HashMap::new();
        for row in rows {
            let (id_bytes, created_at_ms) = row?;
            let Ok(arr) = <[u8; 16]>::try_from(id_bytes.as_slice()) else {
                continue;
            };
            let id = BufferId::from_uuid(Uuid::from_bytes(arr));
            out.entry(id).or_default().push(created_at_ms);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_buffer::Buffer;
    use continuity_text::{EditOp, Position};

    #[test]
    fn empty_store_returns_no_lanes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.db")).unwrap();
        let lanes = store
            .load_buffer_history_timeline(BufferListFilter::ActiveOnly)
            .unwrap();
        assert!(lanes.is_empty());
    }

    #[test]
    fn title_and_preview_are_display_clipped() {
        let long = format!("## {}\nsecond", "a".repeat(120));
        let (title, preview) = derive_title_and_preview(Some(&long));
        let title = title.unwrap();
        assert!(title.ends_with('…'));
        assert!(!title.starts_with('#'));
        assert!(title.chars().count() <= crate::buffer_listing::BUFFER_RECORD_TITLE_MAX_CHARS);
        let preview = preview.unwrap();
        assert!(preview.lines().next().unwrap().ends_with('…'));
        assert!(preview.lines().count() <= PREVIEW_MAX_LINES);
    }

    #[test]
    fn whitespace_content_has_no_title_or_preview() {
        assert_eq!(derive_title_and_preview(Some(" \n\t\n")), (None, None));
    }

    #[test]
    fn content_counts_match_rope_line_and_char_conventions() {
        assert_eq!(derive_content_counts(None), (0, 0));
        assert_eq!(derive_content_counts(Some("")), (1, 0));
        assert_eq!(derive_content_counts(Some("one")), (1, 3));
        assert_eq!(derive_content_counts(Some("one\n")), (2, 4));
        assert_eq!(derive_content_counts(Some("one\nβ")), (2, 5));
    }

    #[test]
    fn lanes_carry_snapshot_timestamps_in_ascending_order() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.db")).unwrap();
        let mut buf = Buffer::from_text("one");
        let id = buf.id();
        store.save_snapshot(id, &buf.snapshot()).unwrap();
        buf.apply(&EditOp::insert(Position::new(0, 3), "!"))
            .unwrap();
        store.save_snapshot(id, &buf.snapshot()).unwrap();

        let lanes = store
            .load_buffer_history_timeline(BufferListFilter::ActiveOnly)
            .unwrap();
        assert_eq!(lanes.len(), 1);
        let lane = &lanes[0];
        assert_eq!(lane.record.id, id);
        assert_eq!(lane.snapshot_times_ms.len(), 2);
        assert!(lane.snapshot_times_ms.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(lane.line_count, 1);
        assert_eq!(lane.char_count, 4);
    }

    #[test]
    fn lanes_sorted_by_last_touched_desc() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.db")).unwrap();
        let b1 = Buffer::from_text("a");
        let b2 = Buffer::from_text("b");
        store.save_snapshot(b1.id(), &b1.snapshot()).unwrap();
        store.save_snapshot(b2.id(), &b2.snapshot()).unwrap();
        store.touch_buffer(b1.id(), i64::MAX).unwrap();
        let lanes = store
            .load_buffer_history_timeline(BufferListFilter::ActiveOnly)
            .unwrap();
        assert_eq!(lanes.len(), 2);
        assert_eq!(lanes[0].record.id, b1.id());
        assert_eq!(lanes[1].record.id, b2.id());
    }

    #[test]
    fn filter_partitions_active_and_trashed() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.db")).unwrap();
        let kept = Buffer::from_text("kept");
        let deleted = Buffer::from_text("deleted");
        store.save_snapshot(kept.id(), &kept.snapshot()).unwrap();
        store
            .save_snapshot(deleted.id(), &deleted.snapshot())
            .unwrap();
        store.move_to_trash(deleted.id(), 1_000, 7).unwrap();

        let active = store
            .load_buffer_history_timeline(BufferListFilter::ActiveOnly)
            .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].record.id, kept.id());
        // Dots for the trashed buffer must not appear in the
        // active-only result.
        assert_eq!(active[0].snapshot_times_ms.len(), 1);

        let trashed = store
            .load_buffer_history_timeline(BufferListFilter::TrashedOnly)
            .unwrap();
        assert_eq!(trashed.len(), 1);
        assert_eq!(trashed[0].record.id, deleted.id());
        assert!(trashed[0].record.is_trashed);
        assert_eq!(trashed[0].snapshot_times_ms.len(), 1);

        let all = store
            .load_buffer_history_timeline(BufferListFilter::All)
            .unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn empty_no_snapshot_no_edit_no_file_rows_are_filtered_out() {
        // Buffers with no edits, no snapshots, and no file
        // association are the residue of `tab.new` sessions and are
        // now hidden from the timeline (and the previous-buffer
        // browser overlay) so they stop accumulating as clutter.
        // The startup `purge_orphan_buffers` sweep eventually
        // removes them from the DB entirely.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.db")).unwrap();
        let id = BufferId::new();
        store.upsert_buffer(id, 100, 100).unwrap();
        let lanes = store
            .load_buffer_history_timeline(BufferListFilter::ActiveOnly)
            .unwrap();
        assert!(lanes.is_empty());
    }
}
