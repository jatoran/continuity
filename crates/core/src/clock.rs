//! Clock abstraction.
//!
//! Production code uses [`SystemClock`]. Tests inject [`FakeClock`] from
//! `continuity-test-support`, which implements this same trait.

use std::time::{SystemTime, UNIX_EPOCH};

/// Source of "now" in Unix milliseconds.
///
/// The trait is `Send + Sync` so it can travel inside an `Arc<dyn Clock>` to
/// the editor core thread.
pub trait Clock: Send + Sync {
    /// Return the current time in milliseconds since the Unix epoch.
    fn now_ms(&self) -> i64;
}

/// Real wall-clock time.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_clock_is_positive() {
        assert!(SystemClock.now_ms() > 1_700_000_000_000);
    }
}
