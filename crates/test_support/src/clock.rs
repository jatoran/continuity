//! Test-only [`FakeClock`] that implements [`continuity_core::Clock`].

use std::sync::atomic::{AtomicI64, Ordering};

use continuity_core::Clock;

/// Test clock that returns a controlled value, advanced by `tick_ms`.
#[derive(Debug, Default)]
pub struct FakeClock {
    now: AtomicI64,
}

impl FakeClock {
    /// Construct a clock starting at `start_ms`.
    #[must_use]
    pub fn at(start_ms: i64) -> Self {
        Self {
            now: AtomicI64::new(start_ms),
        }
    }

    /// Advance the clock by `delta_ms`. Returns the new value.
    pub fn tick_ms(&self, delta_ms: i64) -> i64 {
        self.now.fetch_add(delta_ms, Ordering::Relaxed) + delta_ms
    }
}

impl Clock for FakeClock {
    fn now_ms(&self) -> i64 {
        self.now.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_clock_returns_set_value() {
        let c = FakeClock::at(42);
        assert_eq!(c.now_ms(), 42);
    }

    #[test]
    fn fake_clock_advances() {
        let c = FakeClock::at(100);
        let next = c.tick_ms(7);
        assert_eq!(next, 107);
        assert_eq!(c.now_ms(), 107);
    }
}
