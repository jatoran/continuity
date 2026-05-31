//! UI-side wrapper for the shared running-summary registry.
//!
//! The registry itself lives in [`continuity_trace`] (a leaf crate
//! every emitter can depend on) so `core` and `persist` labels feed
//! the same histograms as UI labels. This module owns the periodic
//! flush — only the UI thread runs the `WM_TIMER` that drives it.
//!
//! ## Crash safety
//!
//! Each flush writes a cumulative snapshot since process start;
//! counters never reset. A `kill -9` between flushes loses up to
//! `CONTINUITY_TRACE_SUMMARY_MS` of summary updates but the last
//! snapshot already on disk reflects every event up to that point.
//!
//! ## Hot-path discipline
//!
//! Recording is gated by [`crate::paint_trace::is_trace_enabled`]
//! before this module is entered; the leaf crate's `record` is a
//! read-lock + atomic adds (~5 atomics per event). The periodic flush
//! runs on the UI thread inside the `WM_TIMER` handler and snapshots
//! process resource counters at the same cadence.

use std::time::Duration;

/// Default flush cadence in milliseconds when
/// `CONTINUITY_TRACE_SUMMARY_MS` is not set.
pub(crate) const DEFAULT_FLUSH_INTERVAL_MS: u64 = 2_000;
/// Minimum honoured cadence.
pub(crate) const MIN_FLUSH_INTERVAL_MS: u64 = 250;

/// Record one observation. Thin wrapper around the leaf-crate registry
/// so a future refactor can intercept here.
#[inline]
pub(crate) fn record(label: &str, dur_us: u64) {
    continuity_trace::record(label, dur_us);
}

/// Called from each window's `WM_TIMER` dispatch when the
/// `TRACE_SUMMARY_TIMER_ID` fires. Iterates the registry and emits one
/// `event:running_summary` line per label via the standard trace
/// emission path. Multiple windows firing the same timer is harmless —
/// each tick emits a fresh snapshot; a downstream summarizer reads the
/// last line per label.
///
/// No-op when tracing is disabled or the registry is empty.
pub(crate) fn tick() {
    if !crate::paint_trace::is_trace_enabled() {
        return;
    }
    continuity_trace::flush_all(crate::paint_trace::log_event);
    // Snapshot process-wide resource counters at the same cadence so a
    // long-running session has periodic visibility into RSS / GDI /
    // handle pressure without a separate timer.
    crate::process_trace::emit_snapshot();
}

/// Resolve the flush cadence from `CONTINUITY_TRACE_SUMMARY_MS`. Values
/// below [`MIN_FLUSH_INTERVAL_MS`] are clamped up; `0` disables flushes.
pub(crate) fn flush_interval() -> Option<Duration> {
    let raw = std::env::var("CONTINUITY_TRACE_SUMMARY_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_FLUSH_INTERVAL_MS);
    if raw == 0 {
        return None;
    }
    Some(Duration::from_millis(raw.max(MIN_FLUSH_INTERVAL_MS)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flush_interval_default_and_clamped() {
        std::env::remove_var("CONTINUITY_TRACE_SUMMARY_MS");
        assert_eq!(
            flush_interval(),
            Some(Duration::from_millis(DEFAULT_FLUSH_INTERVAL_MS))
        );
        std::env::set_var("CONTINUITY_TRACE_SUMMARY_MS", "0");
        assert_eq!(flush_interval(), None);
        std::env::set_var("CONTINUITY_TRACE_SUMMARY_MS", "50");
        assert_eq!(
            flush_interval(),
            Some(Duration::from_millis(MIN_FLUSH_INTERVAL_MS))
        );
        std::env::remove_var("CONTINUITY_TRACE_SUMMARY_MS");
    }
}
