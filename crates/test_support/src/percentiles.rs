//! Percentile summaries for perf-gate sample series.
//!
//! Every Phase 17.9 timing-distribution gate computes the same shape:
//! collect N `Duration` samples, sort, extract p50 / p95 / p99 / p99.9,
//! print the distribution, assert p99 within budget. §B4 added p99.9
//! tracking and the p99/p50 jitter ratio to catch sporadic outliers
//! that p99 alone can hide.
//!
//! Use [`Percentiles::from_samples`] to compute the distribution, the
//! [`std::fmt::Display`] impl to print it, and [`assert_within_budget`]
//! to gate it. The assertion fails when either p99 exceeds the budget
//! or p99.9 exceeds 2× the budget (the variance check from §B4).

use std::fmt;
use std::time::Duration;

/// Percentile summary of a perf-gate sample series.
///
/// p50 / p95 / p99 / p99.9 / max plus the p99/p50 jitter ratio.
/// Sample counts below 1000 produce a degenerate p99.9 (clamped to
/// the last sample) — gates that care about real p99.9 separation
/// should collect ≥ 1000 samples.
#[derive(Clone, Copy, Debug)]
pub struct Percentiles {
    /// 50th percentile (median).
    pub p50: Duration,
    /// 95th percentile.
    pub p95: Duration,
    /// 99th percentile — the headline gate metric.
    pub p99: Duration,
    /// 99.9th percentile — the variance-tail tracking metric from §B4.
    pub p99_9: Duration,
    /// Maximum observed sample.
    pub max: Duration,
    /// Number of samples the distribution was computed over.
    pub sample_count: usize,
}

impl Percentiles {
    /// Compute percentiles from a slice of `Duration` samples. Sorts
    /// `samples` in place.
    ///
    /// # Panics
    ///
    /// Panics if `samples` is empty.
    #[must_use]
    pub fn from_samples(samples: &mut [Duration]) -> Self {
        assert!(!samples.is_empty(), "no samples");
        samples.sort_unstable();
        let n = samples.len();
        let q = |frac: f64| -> Duration {
            let idx = ((n as f64) * frac) as usize;
            samples[idx.min(n - 1)]
        };
        Self {
            p50: q(0.50),
            p95: q(0.95),
            p99: q(0.99),
            p99_9: q(0.999),
            max: samples[n - 1],
            sample_count: n,
        }
    }

    /// Ratio of p99 to p50. Catches sporadic outliers that mean-
    /// targets miss — a healthy distribution has jitter near 1×, a
    /// pathological one many ×.
    #[must_use]
    pub fn jitter(&self) -> f64 {
        let p50_ns = self.p50.as_nanos().max(1) as f64;
        self.p99.as_nanos() as f64 / p50_ns
    }
}

impl fmt::Display for Percentiles {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        write!(
            f,
            "p50={:.3}ms p95={:.3}ms p99={:.3}ms p99.9={:.3}ms max={:.3}ms \
             jitter={:.2}× n={}",
            ms(self.p50),
            ms(self.p95),
            ms(self.p99),
            ms(self.p99_9),
            ms(self.max),
            self.jitter(),
            self.sample_count,
        )
    }
}

/// Assert the distribution fits within a perf-gate budget.
///
/// Two assertions, both from §B4 of Phase 17.9:
/// 1. p99 ≤ `p99_budget`. The headline budget every gate publishes.
/// 2. p99.9 ≤ 2 × `p99_budget`. Variance-tail guard — catches outlier
///    spikes that the p99 sample lets through.
///
/// On failure, the panic message includes the full distribution
/// (via the `Display` impl) so the regression source is visible
/// without rerunning.
///
/// # Panics
///
/// Panics on either assertion failure. The labelled message names
/// which budget was violated.
pub fn assert_within_budget(p: &Percentiles, p99_budget: Duration, label: &str) {
    if let Ok(dir) = std::env::var("CONTINUITY_PERF_LOG_DIR") {
        log_to_perf_dir(&dir, label, p, p99_budget);
    }
    assert!(
        p.p99 <= p99_budget,
        "{label}: p99={:?} exceeds budget {:?} ({p})",
        p.p99,
        p99_budget,
    );
    let p99_9_budget = p99_budget * 2;
    assert!(
        p.p99_9 <= p99_9_budget,
        "{label}: p99.9={:?} exceeds 2× budget {:?} ({p})",
        p.p99_9,
        p99_9_budget,
    );
}

/// Write a single-line JSON record for `label`'s percentiles into
/// `<dir>/<sanitized_label>.json`. Used by `cargo xtask perf-snapshot`
/// (Phase 17.9 §F1) to collect per-gate stats without modifying the
/// individual gate tests. Best-effort: I/O errors are logged to
/// `eprintln!` and ignored so a flaky filesystem can't poison a perf
/// run.
fn log_to_perf_dir(dir: &str, label: &str, p: &Percentiles, p99_budget: Duration) {
    let sanitized: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let path = std::path::Path::new(dir).join(format!("{sanitized}.json"));
    let us = |d: Duration| d.as_micros();
    let body = format!(
        "{{\"label\":\"{label}\",\"p50_us\":{},\"p95_us\":{},\"p99_us\":{},\"p99_9_us\":{},\"max_us\":{},\"p99_budget_us\":{},\"jitter\":{:.6},\"sample_count\":{}}}\n",
        us(p.p50),
        us(p.p95),
        us(p.p99),
        us(p.p99_9),
        us(p.max),
        us(p99_budget),
        p.jitter(),
        p.sample_count,
    );
    if let Err(e) = std::fs::create_dir_all(dir) {
        eprintln!("perf-log: create_dir {dir}: {e}");
        return;
    }
    if let Err(e) = std::fs::write(&path, body) {
        eprintln!("perf-log: write {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples_ms(ms: &[u64]) -> Vec<Duration> {
        ms.iter().map(|m| Duration::from_millis(*m)).collect()
    }

    #[test]
    fn percentiles_basic() {
        let mut s = samples_ms(&[1, 1, 1, 1, 1, 1, 1, 1, 1, 100]);
        let p = Percentiles::from_samples(&mut s);
        assert_eq!(p.p50, Duration::from_millis(1));
        assert_eq!(p.p99, Duration::from_millis(100));
        assert_eq!(p.max, Duration::from_millis(100));
        assert_eq!(p.sample_count, 10);
    }

    #[test]
    fn jitter_is_p99_over_p50() {
        // 99 fast + 1 slow → p99 lands on the slow sample.
        let mut s = samples_ms(&[1; 99]);
        s.push(Duration::from_millis(10));
        let p = Percentiles::from_samples(&mut s);
        assert!(
            (p.jitter() - 10.0).abs() < 0.5,
            "jitter ≈ 10×, got {}",
            p.jitter()
        );
    }

    #[test]
    fn assert_passes_when_within_budget() {
        let mut s = samples_ms(&[1; 100]);
        let p = Percentiles::from_samples(&mut s);
        assert_within_budget(&p, Duration::from_millis(5), "test");
    }

    #[test]
    #[should_panic(expected = "p99=")]
    fn assert_fails_on_p99_breach() {
        let mut s = samples_ms(&[10; 100]);
        let p = Percentiles::from_samples(&mut s);
        assert_within_budget(&p, Duration::from_millis(5), "test");
    }

    #[test]
    #[should_panic(expected = "p99.9=")]
    fn assert_fails_on_p99_9_breach() {
        // 999 fast (≤ p99 budget) + 1 outlier (> 2× budget) → p99
        // passes, p99.9 fails.
        let mut s = samples_ms(&[1; 999]);
        s.push(Duration::from_millis(200));
        let p = Percentiles::from_samples(&mut s);
        assert_within_budget(&p, Duration::from_millis(50), "test");
    }

    #[test]
    fn display_includes_jitter_and_n() {
        let mut s = samples_ms(&[1; 100]);
        let p = Percentiles::from_samples(&mut s);
        let out = format!("{p}");
        assert!(out.contains("p99=1.000ms"), "{out}");
        assert!(out.contains("jitter="), "{out}");
        assert!(out.contains("n=100"), "{out}");
    }
}
