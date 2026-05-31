//! Phase I2 — daily metrics persistence: WPM samples, keystrokes,
//! characters typed/deleted, active milliseconds. One row per
//! local-calendar day, atomically merged via `INSERT … ON CONFLICT`.
//!
//! Thread ownership: every method is on [`Store`], whose connection
//! lives on the persistence thread. Callers reach these through the
//! dedicated [`crate::PersistMessage`] variants
//! (`RecordMetricsDelta` / `LoadMetricsRange` / `PurgeMetrics`).
//!
//! Day strings are caller-supplied (typically `YYYY-MM-DD` in the
//! user's local timezone). The persistence layer treats them as
//! opaque primary keys — the calendar bucketing decision lives one
//! layer up where wall-clock conversions are policy.

use continuity_buffer::BufferId;
use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

use crate::buffer_listing::first_non_empty_trimmed_line;
use crate::checksum::fnv1a_64;
use crate::store::{MetricsDailyDelta, MetricsDailyRow, Store, TopBufferRow};
use crate::Error;

impl Store {
    /// Phase I2: merge `delta` into today's `metrics_daily` row.
    /// Creates the row if missing. `wpm_peak` is `max`-merged;
    /// every other counter is added; `wpm_sample` (when set) is
    /// folded into `wpm_sum` + `wpm_samples` so the rolling average
    /// is a cheap two-column division.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error::Sqlite`].
    pub fn record_metrics_delta(&self, delta: &MetricsDailyDelta) -> Result<(), Error> {
        let sample = delta.wpm_sample.unwrap_or(0);
        let samples_incr: u64 = u64::from(delta.wpm_sample.is_some());
        self.conn().execute(
            "INSERT INTO metrics_daily
                 (day_iso, keystrokes, chars_typed, chars_deleted, active_ms,
                  wpm_peak, wpm_sum, wpm_samples, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(day_iso) DO UPDATE SET
                 keystrokes    = keystrokes    + excluded.keystrokes,
                 chars_typed   = chars_typed   + excluded.chars_typed,
                 chars_deleted = chars_deleted + excluded.chars_deleted,
                 active_ms     = active_ms     + excluded.active_ms,
                 wpm_peak      = MAX(wpm_peak, excluded.wpm_peak),
                 wpm_sum       = wpm_sum       + excluded.wpm_sum,
                 wpm_samples   = wpm_samples   + excluded.wpm_samples,
                 updated_at    = excluded.updated_at",
            params![
                delta.day_iso,
                delta.keystrokes as i64,
                delta.chars_typed as i64,
                delta.chars_deleted as i64,
                delta.active_ms as i64,
                i64::from(sample),
                i64::from(sample),
                samples_incr as i64,
                delta.now_ms,
            ],
        )?;
        Ok(())
    }

    /// Phase I2: load every metric row inside the inclusive ISO-date
    /// window `[start_day_iso, end_day_iso]`, ordered ascending by day.
    /// String comparison on `YYYY-MM-DD` is exactly chronological.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error::Sqlite`].
    pub fn load_metrics_range(
        &self,
        start_day_iso: &str,
        end_day_iso: &str,
    ) -> Result<Vec<MetricsDailyRow>, Error> {
        let mut stmt = self.conn().prepare(
            "SELECT day_iso, keystrokes, chars_typed, chars_deleted, active_ms,
                    wpm_peak, wpm_sum, wpm_samples, updated_at
             FROM metrics_daily
             WHERE day_iso >= ?1 AND day_iso <= ?2
             ORDER BY day_iso ASC",
        )?;
        let rows = stmt.query_map(params![start_day_iso, end_day_iso], |r| {
            Ok(MetricsDailyRow {
                day_iso: r.get(0)?,
                keystrokes: r.get::<_, i64>(1)? as u64,
                chars_typed: r.get::<_, i64>(2)? as u64,
                chars_deleted: r.get::<_, i64>(3)? as u64,
                active_ms: r.get::<_, i64>(4)? as u64,
                wpm_peak: r.get::<_, i64>(5)? as u32,
                wpm_sum: r.get::<_, i64>(6)? as u64,
                wpm_samples: r.get::<_, i64>(7)? as u64,
                updated_at_ms: r.get(8)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Phase I2: load a single day's row, or `None` if no events were
    /// recorded that day.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error::Sqlite`].
    pub fn load_metrics_day(&self, day_iso: &str) -> Result<Option<MetricsDailyRow>, Error> {
        let row = self
            .conn()
            .query_row(
                "SELECT day_iso, keystrokes, chars_typed, chars_deleted, active_ms,
                        wpm_peak, wpm_sum, wpm_samples, updated_at
                 FROM metrics_daily
                 WHERE day_iso = ?1",
                params![day_iso],
                |r| {
                    Ok(MetricsDailyRow {
                        day_iso: r.get(0)?,
                        keystrokes: r.get::<_, i64>(1)? as u64,
                        chars_typed: r.get::<_, i64>(2)? as u64,
                        chars_deleted: r.get::<_, i64>(3)? as u64,
                        active_ms: r.get::<_, i64>(4)? as u64,
                        wpm_peak: r.get::<_, i64>(5)? as u32,
                        wpm_sum: r.get::<_, i64>(6)? as u64,
                        wpm_samples: r.get::<_, i64>(7)? as u64,
                        updated_at_ms: r.get(8)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Phase I2: drop every row in `metrics_daily`. Returns the number
    /// of rows removed. Backs the user-facing `metrics.purge` command.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error::Sqlite`].
    pub fn purge_metrics(&self) -> Result<usize, Error> {
        let n = self.conn().execute("DELETE FROM metrics_daily", [])?;
        Ok(n)
    }

    /// Phase I2: rank buffers by edit-log row count inside the half-open
    /// millisecond window `[start_ms, end_ms)` and return the top
    /// `limit` rows, descending by count.
    ///
    /// Powers the §I2 "Top buffers by edit count this week" surface
    /// on the metrics panel. The query joins against `buffer_edits.ts`
    /// directly so no schema change is needed; the existing
    /// `idx_edits_buffer_revision` index does not help, but the table
    /// scan is bounded to one week's edits in the common case.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error::Sqlite`].
    pub fn load_top_buffers_by_edits(
        &self,
        start_ms: i64,
        end_ms: i64,
        limit: usize,
    ) -> Result<Vec<TopBufferRow>, Error> {
        let mut stmt = self.conn().prepare(
            "WITH ranked AS (
                 SELECT buffer_id, COUNT(*) AS edit_count
                 FROM buffer_edits
                 WHERE ts >= ?1 AND ts < ?2
                 GROUP BY buffer_id
                 ORDER BY edit_count DESC
                 LIMIT ?3
             )
             SELECT ranked.buffer_id,
                    ranked.edit_count,
                    b.file_path,
                    s.content_blob,
                    s.checksum
             FROM ranked
             LEFT JOIN buffers b
               ON b.id = ranked.buffer_id
             LEFT JOIN buffer_snapshots s
               ON s.buffer_id = ranked.buffer_id
              AND s.revision = (
                    SELECT MAX(revision)
                      FROM buffer_snapshots
                     WHERE buffer_id = ranked.buffer_id
              )
             ORDER BY ranked.edit_count DESC",
        )?;
        let rows = stmt.query_map(
            params![start_ms, end_ms, i64::try_from(limit).unwrap_or(i64::MAX)],
            |r| {
                let blob: Vec<u8> = r.get(0)?;
                let count: i64 = r.get(1)?;
                let file_path: Option<String> = r.get(2)?;
                let snapshot_blob: Option<Vec<u8>> = r.get(3)?;
                let checksum: Option<i64> = r.get(4)?;
                Ok((blob, count, file_path, snapshot_blob, checksum))
            },
        )?;
        let mut out = Vec::new();
        for row in rows {
            let (blob, count, file_path, snapshot_blob, checksum) = row?;
            let Some(arr) = <[u8; 16]>::try_from(blob.as_slice()).ok() else {
                // Malformed blob shouldn't be fatal — skip it so other
                // ranks still surface. Buffer_id is BLOB NOT NULL with
                // a UUID-bytes contract everywhere else in the schema.
                continue;
            };
            out.push(TopBufferRow {
                buffer_id: BufferId::from_uuid(Uuid::from_bytes(arr)),
                title: decode_top_buffer_title(snapshot_blob.as_deref(), checksum),
                file_path,
                edit_count: u64::try_from(count).unwrap_or(0),
            });
        }
        Ok(out)
    }
}

fn decode_top_buffer_title(blob: Option<&[u8]>, checksum: Option<i64>) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(
        day: &str,
        keys: u64,
        typed: u64,
        deleted: u64,
        ms: u64,
        sample: Option<u32>,
    ) -> MetricsDailyDelta {
        MetricsDailyDelta {
            day_iso: day.to_string(),
            keystrokes: keys,
            chars_typed: typed,
            chars_deleted: deleted,
            active_ms: ms,
            wpm_sample: sample,
            now_ms: 1_700_000_000_000,
        }
    }

    #[test]
    fn upsert_creates_then_accumulates() {
        let store = Store::open_in_memory().unwrap();
        store
            .record_metrics_delta(&delta("2026-05-12", 10, 8, 2, 5_000, Some(40)))
            .unwrap();
        store
            .record_metrics_delta(&delta("2026-05-12", 5, 4, 1, 2_000, Some(60)))
            .unwrap();

        let row = store.load_metrics_day("2026-05-12").unwrap().unwrap();
        assert_eq!(row.keystrokes, 15);
        assert_eq!(row.chars_typed, 12);
        assert_eq!(row.chars_deleted, 3);
        assert_eq!(row.active_ms, 7_000);
        assert_eq!(row.wpm_peak, 60); // max-merged
        assert_eq!(row.wpm_samples, 2);
        assert_eq!(row.wpm_sum, 100);
    }

    #[test]
    fn wpm_sample_none_does_not_increment_samples() {
        let store = Store::open_in_memory().unwrap();
        store
            .record_metrics_delta(&delta("2026-05-12", 1, 1, 0, 100, None))
            .unwrap();
        let row = store.load_metrics_day("2026-05-12").unwrap().unwrap();
        assert_eq!(row.wpm_samples, 0);
        assert_eq!(row.wpm_sum, 0);
        assert_eq!(row.wpm_peak, 0);
    }

    #[test]
    fn load_range_returns_chronological_rows() {
        let store = Store::open_in_memory().unwrap();
        store
            .record_metrics_delta(&delta("2026-05-12", 1, 0, 0, 0, None))
            .unwrap();
        store
            .record_metrics_delta(&delta("2026-05-10", 1, 0, 0, 0, None))
            .unwrap();
        store
            .record_metrics_delta(&delta("2026-05-11", 1, 0, 0, 0, None))
            .unwrap();
        store
            .record_metrics_delta(&delta("2026-04-30", 1, 0, 0, 0, None))
            .unwrap();

        let rows = store
            .load_metrics_range("2026-05-10", "2026-05-12")
            .unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].day_iso, "2026-05-10");
        assert_eq!(rows[1].day_iso, "2026-05-11");
        assert_eq!(rows[2].day_iso, "2026-05-12");
    }

    #[test]
    fn load_day_returns_none_for_missing() {
        let store = Store::open_in_memory().unwrap();
        let r = store.load_metrics_day("2026-05-12").unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn purge_drops_every_row() {
        let store = Store::open_in_memory().unwrap();
        store
            .record_metrics_delta(&delta("2026-05-12", 1, 1, 1, 1, Some(20)))
            .unwrap();
        store
            .record_metrics_delta(&delta("2026-05-11", 1, 1, 1, 1, Some(30)))
            .unwrap();
        assert_eq!(store.load_metrics_range("0000", "9999").unwrap().len(), 2);
        let n = store.purge_metrics().unwrap();
        assert_eq!(n, 2);
        assert_eq!(store.load_metrics_range("0000", "9999").unwrap().len(), 0);
    }

    #[test]
    fn purge_expired_does_not_touch_metrics_daily() {
        // §I2 decision: `metrics_daily` is excluded from the trash
        // retention flow. Verify that calling `purge_expired` on a
        // store containing both an expired trashed buffer and a
        // metrics-daily row leaves the metrics row intact.
        let store = Store::open_in_memory().unwrap();
        let buffer_id = BufferId::new();
        store.upsert_buffer(buffer_id, 0, 0).unwrap();
        // Move to trash with retention = 0 ⇒ immediately purgeable.
        store.move_to_trash(buffer_id, 1_000, 0).unwrap();
        store
            .record_metrics_delta(&delta("2026-05-12", 7, 6, 1, 4_000, Some(55)))
            .unwrap();

        let purged = store.purge_expired(2_000).unwrap();
        assert_eq!(purged, 1, "the trashed buffer should have been purged");

        // The metrics row still exists.
        let row = store.load_metrics_day("2026-05-12").unwrap().unwrap();
        assert_eq!(row.keystrokes, 7);
        assert_eq!(row.wpm_peak, 55);
    }

    #[test]
    fn top_buffers_ranks_by_edit_count_inside_window() {
        let store = Store::open_in_memory().unwrap();
        let busy = BufferId::new();
        let quiet = BufferId::new();
        let stale = BufferId::new();
        for id in [busy, quiet, stale] {
            store.upsert_buffer(id, 0, 0).unwrap();
        }

        // 5 in-window edits on `busy`, 2 on `quiet`, 3 on `stale` but
        // before the window. Window is [10_000, 20_000).
        for seq in 0..5 {
            let row = edit_row(busy, seq, 12_000 + seq as i64);
            store.append_edit(&row).unwrap();
        }
        for seq in 0..2 {
            let row = edit_row(quiet, seq, 15_000 + seq as i64);
            store.append_edit(&row).unwrap();
        }
        for seq in 0..3 {
            let row = edit_row(stale, seq, 5_000 + seq as i64);
            store.append_edit(&row).unwrap();
        }

        let top = store.load_top_buffers_by_edits(10_000, 20_000, 10).unwrap();
        assert_eq!(top.len(), 2, "stale must not appear (outside window)");
        assert_eq!(top[0].buffer_id, busy);
        assert_eq!(top[0].edit_count, 5);
        assert_eq!(top[1].buffer_id, quiet);
        assert_eq!(top[1].edit_count, 2);

        // `limit` is honored.
        let just_one = store.load_top_buffers_by_edits(10_000, 20_000, 1).unwrap();
        assert_eq!(just_one.len(), 1);
        assert_eq!(just_one[0].buffer_id, busy);

        // Empty window produces empty result.
        let empty = store.load_top_buffers_by_edits(0, 100, 10).unwrap();
        assert!(empty.is_empty());
    }

    fn edit_row(buffer_id: BufferId, seq: u64, ts_ms: i64) -> crate::store::EditRow {
        crate::store::EditRow {
            buffer_id,
            seq,
            revision: continuity_buffer::Revision(seq + 1),
            ts_ms,
            op_kind: "insert".to_string(),
            range_start_line: Some(0),
            range_start_byte: Some(0),
            range_end_line: None,
            range_end_byte: None,
            inserted_text: Some("x".to_string()),
            removed_text: None,
            selections_before_json: None,
            selections_after_json: None,
            undo_group_id: None,
            checksum_after: 0,
        }
    }
}
