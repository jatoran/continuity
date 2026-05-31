#![warn(missing_docs)]
//! Process-wide trace summary registry.
//!
//! Each producer crate (`ui::paint_trace`, `core::trace`,
//! `persist::trace`) still owns its own TSV file sink. They all open
//! `CONTINUITY_UI_TRACE` in append mode and rely on the OS to
//! interleave line writes. **What this crate adds** is the shared
//! per-label histogram registry that the UI's
//! `paint_trace_summary::tick()` flushes every
//! `CONTINUITY_TRACE_SUMMARY_MS`.
//!
//! Producers call [`record(label, dur_us)`](record) immediately after
//! their own `emit_line`. The registry is process-global, so
//! `core_apply_selection_edit` and `persist_loop_append_edit` end up
//! in the same percentile table as `edit_apply` and `WM_PAINT`.
//!
//! The implementation is lock-free on the hot path: the registry is a
//! `RwLock<HashMap<…>>`, but per-label `LabelHistogram` updates are
//! atomic adds against `&'static` `Box::leak`ed buckets. The lock is
//! only taken for read on the fast (already-registered) path and for
//! write on the rare (first sighting of a new label) path.
//!
//! ## Why a separate crate
//!
//! Layer-graph cleanliness. Lifting the registry to a leaf crate lets
//! `core` and `persist` record into it without taking a dependency on
//! `ui` (which would invert the import direction). `ui::paint_trace`
//! still owns the periodic flush — it's the only crate that runs the
//! Win32 timer.

mod event_sink;

pub use event_sink::{is_enabled, log_event, log_event_us, sync_start_time};

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

/// Bucket upper bounds (microseconds, exclusive). A duration `d` falls
/// in bucket `i` when `d < BUCKET_BOUNDS_US[i]` and (if `i > 0`)
/// `d >= BUCKET_BOUNDS_US[i - 1]`. Anything `>= BUCKET_BOUNDS_US.last()`
/// lands in the overflow bucket at index `BUCKET_COUNT - 1`.
pub const BUCKET_BOUNDS_US: [u64; 15] = [
    100, 250, 500, 1_000, 2_000, 4_000, 8_000, 16_000, 32_000, 64_000, 100_000, 250_000, 500_000,
    1_000_000, 2_000_000,
];
/// 15 bounded buckets + one overflow bucket.
pub const BUCKET_COUNT: usize = BUCKET_BOUNDS_US.len() + 1;

const STALL_THRESHOLD_US: u64 = 16_000;
const SEVERE_STALL_THRESHOLD_US: u64 = 100_000;

/// Per-label running histogram. Counters are atomic-relaxed; readers
/// see internally-consistent values, but updates across counters are
/// not transactional.
pub struct LabelHistogram {
    /// 15 bounded + 1 overflow bucket counters.
    pub buckets: [AtomicU64; BUCKET_COUNT],
    /// Total observations.
    pub count: AtomicU64,
    /// Sum of all recorded durations (microseconds).
    pub sum_us: AtomicU64,
    /// Maximum observed duration (microseconds).
    pub max_us: AtomicU64,
    /// Events at or above the 16 ms stall threshold.
    pub stalls: AtomicU64,
    /// Events at or above the 100 ms severe-stall threshold.
    pub stalls100: AtomicU64,
}

impl LabelHistogram {
    const fn new() -> Self {
        Self {
            buckets: [const { AtomicU64::new(0) }; BUCKET_COUNT],
            count: AtomicU64::new(0),
            sum_us: AtomicU64::new(0),
            max_us: AtomicU64::new(0),
            stalls: AtomicU64::new(0),
            stalls100: AtomicU64::new(0),
        }
    }

    fn observe(&self, dur_us: u64) {
        let bucket = bucket_for_us(dur_us);
        self.buckets[bucket].fetch_add(1, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum_us.fetch_add(dur_us, Ordering::Relaxed);
        self.max_us.fetch_max(dur_us, Ordering::Relaxed);
        if dur_us >= SEVERE_STALL_THRESHOLD_US {
            self.stalls100.fetch_add(1, Ordering::Relaxed);
        } else if dur_us >= STALL_THRESHOLD_US {
            self.stalls.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn bucket_for_us(dur_us: u64) -> usize {
    for (i, &bound) in BUCKET_BOUNDS_US.iter().enumerate() {
        if dur_us < bound {
            return i;
        }
    }
    BUCKET_COUNT - 1
}

static LABEL_REGISTRY: OnceLock<RwLock<HashMap<String, &'static LabelHistogram>>> = OnceLock::new();

fn registry() -> &'static RwLock<HashMap<String, &'static LabelHistogram>> {
    LABEL_REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Record one observation against `label`. Lock-free on the hot path
/// once the label has been registered (one `Box::leak` per unique
/// label).
pub fn record(label: &str, dur_us: u64) {
    let reg = registry();
    {
        let read = reg
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(hist) = read.get(label) {
            hist.observe(dur_us);
            return;
        }
    }
    let mut write = reg
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let hist = *write
        .entry(label.to_string())
        .or_insert_with(|| Box::leak(Box::new(LabelHistogram::new())));
    hist.observe(dur_us);
}

/// Iterate the registry and emit one `(label, detail)` pair per
/// recorded label. `emit` is typically the producer crate's
/// `log_event` so the detail lands inside an `event:running_summary`
/// line.
pub fn flush_all(mut emit: impl FnMut(&str, &str)) {
    let reg = registry();
    let read = reg
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for (label, hist) in read.iter() {
        let detail = format_summary_detail(label, hist);
        emit("running_summary", &detail);
    }
}

fn format_summary_detail(label: &str, hist: &LabelHistogram) -> String {
    let n = hist.count.load(Ordering::Relaxed);
    let sum = hist.sum_us.load(Ordering::Relaxed);
    let max = hist.max_us.load(Ordering::Relaxed);
    let stalls = hist.stalls.load(Ordering::Relaxed);
    let stalls100 = hist.stalls100.load(Ordering::Relaxed);
    let mut buckets = [0u64; BUCKET_COUNT];
    for (i, b) in hist.buckets.iter().enumerate() {
        buckets[i] = b.load(Ordering::Relaxed);
    }
    let mean = if n > 0 { sum / n } else { 0 };
    let p50 = percentile_from_buckets(&buckets, n, 50);
    let p95 = percentile_from_buckets(&buckets, n, 95);
    let p99 = percentile_from_buckets(&buckets, n, 99);
    let mut detail = format!(
        "label={label} n={n} sum_us={sum} mean_us={mean} max_us={max} \
         p50_us={p50} p95_us={p95} p99_us={p99} stalls={stalls} stalls100={stalls100}"
    );
    for (i, &b) in buckets.iter().enumerate() {
        let bound_label = if i < BUCKET_BOUNDS_US.len() {
            format!("b_lt_{}us", BUCKET_BOUNDS_US[i])
        } else {
            "b_overflow".to_string()
        };
        detail.push_str(&format!(" {bound_label}={b}"));
    }
    detail
}

fn percentile_from_buckets(buckets: &[u64; BUCKET_COUNT], total: u64, pct: u64) -> u64 {
    if total == 0 {
        return 0;
    }
    let threshold = total.saturating_mul(pct).div_ceil(100);
    let mut cum: u64 = 0;
    for (i, &count) in buckets.iter().enumerate() {
        cum = cum.saturating_add(count);
        if cum >= threshold {
            return BUCKET_BOUNDS_US.get(i).copied().unwrap_or(u64::MAX);
        }
    }
    u64::MAX
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_at_and_just_below_boundary() {
        assert_eq!(bucket_for_us(99), 0);
        assert_eq!(bucket_for_us(100), 1);
        assert_eq!(bucket_for_us(2_000_001), BUCKET_COUNT - 1);
    }

    #[test]
    fn percentile_zero_total_is_zero() {
        let buckets = [0u64; BUCKET_COUNT];
        assert_eq!(percentile_from_buckets(&buckets, 0, 50), 0);
    }

    #[test]
    fn observe_updates_count_sum_max_stalls() {
        let hist = LabelHistogram::new();
        hist.observe(50);
        hist.observe(20_000); // stall
        hist.observe(150_000); // severe
        assert_eq!(hist.count.load(Ordering::Relaxed), 3);
        assert_eq!(hist.sum_us.load(Ordering::Relaxed), 170_050);
        assert_eq!(hist.max_us.load(Ordering::Relaxed), 150_000);
        assert_eq!(hist.stalls.load(Ordering::Relaxed), 1);
        assert_eq!(hist.stalls100.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn record_then_flush_round_trips_one_label() {
        record("leaf_crate.test.label", 250);
        record("leaf_crate.test.label", 750);
        let mut captured: Vec<(String, String)> = Vec::new();
        flush_all(|label, detail| captured.push((label.to_string(), detail.to_string())));
        let line = captured
            .iter()
            .find(|(_, d)| d.contains("label=leaf_crate.test.label"))
            .expect("flush emits the label we recorded");
        assert!(line.1.contains("n=2"));
        assert!(line.1.contains("sum_us=1000"));
    }
}
