//! Phase I2 — WPM rolling-window tracker.
//!
//! Pure data type: no I/O, no clock. Caller feeds millisecond-stamped
//! samples in monotonic order; the tracker reports the WPM (5-char-word
//! convention) over the most-recent `window_ms` of activity.
//!
//! Live design notes:
//!
//! - The WPM convention is the standard typing-test "5 keystrokes = 1
//!   word" model. So `WPM = (chars_in_window * 60_000 / window_ms) / 5`.
//! - Samples older than `window_ms` are pruned on every observe-call;
//!   the queue is bounded by the actual sample rate * window, which at
//!   the editor's input rate is tiny (~ 1k events worst case at a 60 s
//!   window).
//! - Only character-producing keystrokes contribute. The caller decides
//!   what counts — typically `Insert` events.
//!
//! Owning thread: this is a `Send` value with no internal sharing.
//! The UI thread will hold one instance per window and feed it on each
//! observed insert.

use std::collections::VecDeque;

/// Rolling window of recent character-insertion timestamps.
///
/// The default window is 60 s (matches §I2's "rolling 60 s WPM").
#[derive(Debug, Clone)]
pub struct WpmTracker {
    window_ms: u64,
    samples: VecDeque<u64>,
}

impl WpmTracker {
    /// Build a tracker with the supplied rolling-window length.
    ///
    /// `window_ms` is clamped to `>= 1` to avoid divide-by-zero.
    #[must_use]
    pub fn new(window_ms: u64) -> Self {
        Self {
            window_ms: window_ms.max(1),
            samples: VecDeque::new(),
        }
    }

    /// Build a tracker with the §I2 default 60 s window.
    #[must_use]
    pub(crate) fn default_60s() -> Self {
        Self::new(60_000)
    }

    /// Window length in milliseconds.
    #[must_use]
    pub fn window_ms(&self) -> u64 {
        self.window_ms
    }

    /// Number of samples currently retained in the rolling window.
    #[must_use]
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }

    /// Record one keystroke at `now_ms`. Samples must be supplied in
    /// non-decreasing time order; out-of-order samples are dropped to
    /// keep the rolling-window invariant intact.
    pub fn record(&mut self, now_ms: u64) {
        if let Some(&last) = self.samples.back() {
            if now_ms < last {
                return;
            }
        }
        self.samples.push_back(now_ms);
        self.prune_older_than(now_ms);
    }

    /// Compute the WPM as of `now_ms`. Prunes any samples older than
    /// the window, then divides the remaining sample count by the
    /// "5 chars = 1 word" convention scaled to a per-minute rate.
    ///
    /// Returns `0` when no samples are inside the window.
    pub fn wpm_now(&mut self, now_ms: u64) -> u32 {
        self.prune_older_than(now_ms);
        let chars = self.samples.len() as u64;
        if chars == 0 {
            return 0;
        }
        // chars / 5 = words. words * (60_000 / window_ms) = WPM.
        let words_per_minute = chars * 60_000 / (5 * self.window_ms);
        u32::try_from(words_per_minute).unwrap_or(u32::MAX)
    }

    /// Drop every sample older than `now_ms - window_ms`. Idempotent.
    fn prune_older_than(&mut self, now_ms: u64) {
        let cutoff = now_ms.saturating_sub(self.window_ms);
        while let Some(&t) = self.samples.front() {
            if t < cutoff {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    /// Snapshot-style read — does not mutate, but does not prune
    /// either. Used by tests asserting the post-prune state.
    #[must_use]
    pub fn peek_samples(&self) -> &VecDeque<u64> {
        &self.samples
    }
}

impl Default for WpmTracker {
    fn default() -> Self {
        Self::default_60s()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_window_is_60_seconds() {
        let t = WpmTracker::default();
        assert_eq!(t.window_ms(), 60_000);
    }

    #[test]
    fn empty_tracker_reports_zero() {
        let mut t = WpmTracker::default_60s();
        assert_eq!(t.wpm_now(0), 0);
    }

    #[test]
    fn full_window_of_300_chars_is_60_wpm() {
        // 300 chars in a 60 s window = 300 / 5 = 60 words = 60 WPM.
        let mut t = WpmTracker::new(60_000);
        for i in 0..300u64 {
            t.record(i); // 1ms apart — all inside the 60 s window.
        }
        assert_eq!(t.wpm_now(299), 60);
    }

    #[test]
    fn samples_older_than_window_are_pruned() {
        let mut t = WpmTracker::new(1_000);
        for i in 0..10u64 {
            t.record(i * 100);
        }
        // Now jump beyond the window — 10s later — and observe.
        let wpm = t.wpm_now(10_000);
        assert_eq!(wpm, 0);
        assert_eq!(t.sample_count(), 0);
    }

    #[test]
    fn out_of_order_record_is_dropped() {
        let mut t = WpmTracker::new(10_000);
        t.record(500);
        t.record(400); // out of order — ignored.
        t.record(600);
        assert_eq!(t.sample_count(), 2);
    }

    #[test]
    fn record_then_advance_clock_decays_wpm() {
        let mut t = WpmTracker::new(10_000);
        for i in 0..50u64 {
            t.record(i * 100); // 50 chars across 5 s.
        }
        let before = t.wpm_now(5_000);
        // 7 s later — half the samples (those older than t-window) should
        // have fallen out.
        let after = t.wpm_now(12_000);
        assert!(
            after < before,
            "expected decay: before={before} after={after}"
        );
    }

    #[test]
    fn zero_window_is_clamped() {
        let mut t = WpmTracker::new(0);
        assert!(t.window_ms() >= 1);
        t.record(0);
        // wpm_now divides by 5 * window — must not panic.
        let _ = t.wpm_now(0);
    }

    #[test]
    fn short_window_amplifies_per_minute_rate() {
        // 5 chars in 1 s window → 5/5 = 1 word per second → 60 WPM.
        let mut t = WpmTracker::new(1_000);
        for i in 0..5u64 {
            t.record(i);
        }
        assert_eq!(t.wpm_now(4), 60);
    }
}
