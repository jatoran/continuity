//! Daily metrics `PersistClient` methods.
//!
//! Sibling of [`crate::handle`]; every method is a thin wrapper around a
//! [`crate::PersistMessage`] variant. The request channel is the only
//! thread boundary.

use crossbeam_channel::bounded;

use crate::handle::PersistClient;
use crate::message::PersistMessage;
use crate::store::{MetricsDailyDelta, MetricsDailyRow, TopBufferRow};
use crate::Error;

impl PersistClient {
    /// Merge a metrics delta into today's row. Fire-and-forget.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ThreadGone`] when the persist thread has exited.
    pub fn record_metrics_delta(&self, delta: MetricsDailyDelta) -> Result<(), Error> {
        self.sender()
            .send(PersistMessage::RecordMetricsDelta { delta })
            .map_err(|_| Error::ThreadGone)
    }

    /// Synchronously load every metric row inside the inclusive
    /// ISO-date window `[start, end]`.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error`] the thread reports.
    pub fn load_metrics_range(
        &self,
        start_day_iso: String,
        end_day_iso: String,
    ) -> Result<Vec<MetricsDailyRow>, Error> {
        let (tx, rx) = bounded(1);
        self.sender()
            .send(PersistMessage::LoadMetricsRange {
                start_day_iso,
                end_day_iso,
                reply: tx,
            })
            .map_err(|_| Error::ThreadGone)?;
        rx.recv().map_err(|_| Error::ThreadGone)?
    }

    /// Drop every row from `metrics_daily`. Replies with the number of
    /// rows removed.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error`] the thread reports.
    pub fn purge_metrics(&self) -> Result<usize, Error> {
        let (tx, rx) = bounded(1);
        self.sender()
            .send(PersistMessage::PurgeMetrics { reply: tx })
            .map_err(|_| Error::ThreadGone)?;
        rx.recv().map_err(|_| Error::ThreadGone)?
    }

    /// Rank buffers by edit-log row count inside the half-open
    /// millisecond window `[start_ms, end_ms)` and return the top
    /// `limit` rows, descending by count.
    ///
    /// # Errors
    ///
    /// Propagates any [`Error`] the thread reports.
    pub fn load_top_buffers_by_edits(
        &self,
        start_ms: i64,
        end_ms: i64,
        limit: usize,
    ) -> Result<Vec<TopBufferRow>, Error> {
        let (tx, rx) = bounded(1);
        self.sender()
            .send(PersistMessage::LoadTopBuffersByEdits {
                start_ms,
                end_ms,
                limit,
                reply: tx,
            })
            .map_err(|_| Error::ThreadGone)?;
        rx.recv().map_err(|_| Error::ThreadGone)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handle::PersistHandle;

    #[test]
    fn record_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.db");
        let h = PersistHandle::spawn(&path).unwrap();
        let c = h.client();

        c.record_metrics_delta(MetricsDailyDelta {
            day_iso: "2026-05-12".into(),
            keystrokes: 10,
            chars_typed: 8,
            chars_deleted: 2,
            active_ms: 5_000,
            wpm_sample: Some(50),
            now_ms: 1_700_000_000_000,
        })
        .unwrap();

        let rows = c
            .load_metrics_range("2026-05-01".into(), "2026-05-31".into())
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].keystrokes, 10);
        assert_eq!(rows[0].wpm_peak, 50);
    }

    #[test]
    fn purge_returns_row_count() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.db");
        let h = PersistHandle::spawn(&path).unwrap();
        let c = h.client();
        c.record_metrics_delta(MetricsDailyDelta {
            day_iso: "2026-05-12".into(),
            ..Default::default()
        })
        .unwrap();
        let n = c.purge_metrics().unwrap();
        assert_eq!(n, 1);
    }
}
