//! Unified UI-thread motion contract.
//!
//! `Window` owns every scheduler instance. Callers receive plain draw
//! projections and hand them to `continuity_render`; no renderer state is
//! mutated by these helpers.

use continuity_render::SurfaceMotion;

/// Contract duration for structural motion in milliseconds.
pub(crate) const STRUCTURAL_MOTION_MS: u32 = 160;
/// Contract duration for short acknowledgement transients.
pub(crate) const ACK_MOTION_MS: u32 = 180;
/// Offset between simultaneous region transitions.
pub(crate) const STAGGER_OFFSET_MS: u64 = 60;
/// Frame cadence for motion-driven invalidation.
pub(crate) const MOTION_TIMER_MS: u32 = 16;
const STAGGER_BATCH_MS: u64 = 16;
const ENTER_OFFSET_DIP: f32 = -8.0;
const TRANSIENT_OFFSET_DIP: f32 = -3.0;

/// User-resolved motion policy.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct MotionPolicy {
    reduced_motion: bool,
}

impl MotionPolicy {
    /// Build a policy from `[ui].reduced_motion`.
    #[must_use]
    pub(crate) fn new(reduced_motion: bool) -> Self {
        Self { reduced_motion }
    }

    /// Update the reduced-motion flag.
    pub(crate) fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.reduced_motion = reduced_motion;
    }

    /// `true` when animations must be skipped entirely.
    #[must_use]
    pub(crate) fn is_reduced_motion(self) -> bool {
        self.reduced_motion
    }

    /// Schedule a span unless reduced motion suppresses it.
    #[must_use]
    pub(crate) fn schedule(self, started_ms: u64, duration_ms: u32) -> Option<MotionSpan> {
        if self.reduced_motion || duration_ms == 0 {
            None
        } else {
            Some(MotionSpan {
                started_ms,
                duration_ms,
            })
        }
    }
}

/// One delayed/eased motion interval.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) struct MotionSpan {
    /// Monotonic/UI clock tick when the interval begins.
    pub(crate) started_ms: u64,
    /// Duration in milliseconds.
    pub(crate) duration_ms: u32,
}

impl MotionSpan {
    /// Eased progress in `[0, 1]`, or `None` once complete.
    #[must_use]
    pub(crate) fn progress(self, now_ms: u64) -> Option<f32> {
        if self.duration_ms == 0 {
            return None;
        }
        if now_ms < self.started_ms {
            return Some(0.0);
        }
        let elapsed = now_ms.saturating_sub(self.started_ms);
        if elapsed >= u64::from(self.duration_ms) {
            return None;
        }
        let t = elapsed as f32 / self.duration_ms as f32;
        Some(ease_out_cubic(t))
    }

    /// `true` when the delayed interval has not yet finished.
    #[must_use]
    pub(crate) fn is_alive(self, now_ms: u64) -> bool {
        now_ms < self.started_ms.saturating_add(u64::from(self.duration_ms))
    }
}

/// Staggers transitions that are scheduled in the same paint/input batch.
#[derive(Clone, Debug, Default)]
pub(crate) struct StaggerScheduler {
    batch_started_ms: Option<u64>,
    next_slot: u32,
}

impl StaggerScheduler {
    /// Return the start time for the next transition in the current batch.
    pub(crate) fn next_start(&mut self, now_ms: u64) -> u64 {
        let reset = match self.batch_started_ms {
            Some(started) => now_ms.saturating_sub(started) > STAGGER_BATCH_MS,
            None => true,
        };
        if reset {
            self.batch_started_ms = Some(now_ms);
            self.next_slot = 0;
        }
        let delay = u64::from(self.next_slot) * STAGGER_OFFSET_MS;
        self.next_slot = self.next_slot.saturating_add(1);
        now_ms.saturating_add(delay)
    }

    /// Schedule a staggered span unless reduced motion suppresses it.
    pub(crate) fn schedule(
        &mut self,
        policy: MotionPolicy,
        now_ms: u64,
        duration_ms: u32,
    ) -> Option<MotionSpan> {
        let started_ms = self.next_start(now_ms);
        policy.schedule(started_ms, duration_ms)
    }

    /// Clear batch state after reduced-motion toggles or window teardown.
    pub(crate) fn reset(&mut self) {
        self.batch_started_ms = None;
        self.next_slot = 0;
    }
}

/// Cubic ease-out curve used by every non-zero contract motion.
#[must_use]
pub(crate) fn ease_out_cubic(t: f32) -> f32 {
    let u = 1.0 - t.clamp(0.0, 1.0);
    1.0 - u * u * u
}

/// Convert an enter/open progress value to a render projection.
#[must_use]
pub(crate) fn enter_motion(progress: f32) -> SurfaceMotion {
    let p = progress.clamp(0.0, 1.0);
    SurfaceMotion::new(p, ENTER_OFFSET_DIP * (1.0 - p))
}

/// Convert an exit/close progress value to a render projection.
#[must_use]
pub(crate) fn exit_motion(progress: f32) -> SurfaceMotion {
    let p = progress.clamp(0.0, 1.0);
    SurfaceMotion::new(1.0 - p, ENTER_OFFSET_DIP * p)
}

/// Convert a localized value-change progress value to a draw transient.
#[must_use]
pub(crate) fn transient_alpha_and_offset(progress: f32) -> (f32, f32) {
    let p = progress.clamp(0.0, 1.0);
    (1.0 - p, TRANSIENT_OFFSET_DIP * (1.0 - p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduced_motion_schedules_no_span() {
        let policy = MotionPolicy::new(true);
        assert!(policy.schedule(10, STRUCTURAL_MOTION_MS).is_none());
    }

    #[test]
    fn span_progress_uses_ease_out_cubic() {
        let span = MotionSpan {
            started_ms: 100,
            duration_ms: 160,
        };
        let p = span.progress(180).expect("active");
        assert!((p - 0.875).abs() < 0.01);
    }

    #[test]
    fn stagger_offsets_same_batch_by_sixty_ms() {
        let mut scheduler = StaggerScheduler::default();
        assert_eq!(scheduler.next_start(1000), 1000);
        assert_eq!(scheduler.next_start(1000), 1060);
        assert_eq!(scheduler.next_start(1008), 1128);
    }

    #[test]
    fn stagger_resets_after_batch_window() {
        let mut scheduler = StaggerScheduler::default();
        assert_eq!(scheduler.next_start(1000), 1000);
        assert_eq!(scheduler.next_start(1100), 1100);
    }
}
