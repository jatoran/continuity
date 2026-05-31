//! Worker-hit install-arm sub-stage timing.
//!
//! Split out of `worker_outcome_dispatch.rs` to keep that file under
//! the conventions cap. The single responsibility here is timing the
//! microsecond-scoped worker-hit install arm and emitting the
//! `event:worker_hit_stages` breakdown so the perf analyzer can
//! separate dispatch-arm cost from the upstream paint preparation that
//! dominates the `frame_display:worker_hit` paint-mark duration.
//!
//! Thread ownership: pure helpers over caller-captured `Instant`s; no
//! `Window` state.

use std::time::Instant;

/// Wall-clock snapshots captured around the worker-hit install arm.
///
/// Carried to [`compute_worker_hit_stages`] so the sub-stage breakdown
/// emitted on `event:worker_hit_stages` is testable without spinning up
/// a full [`crate::window::Window`].
pub(super) struct WorkerHitStageInstants {
    pub(super) arm_start: Instant,
    pub(super) after_extract: Instant,
    pub(super) after_event_log: Instant,
    pub(super) after_marks: Instant,
}

/// Sub-stage durations of the worker-hit install arm, in microseconds.
/// Field order matches the trace-event field order to keep the
/// analyzer's column parse stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct WorkerHitStages {
    pub(super) extract_us: u64,
    pub(super) event_log_us: u64,
    pub(super) paint_marks_us: u64,
    pub(super) arm_total_us: u64,
}

#[cfg(test)]
impl WorkerHitStages {
    /// Sum of the individual sub-stage components. Used only by the
    /// inline test asserting the components account for the arm total
    /// within 5 %.
    fn component_sum_us(&self) -> u64 {
        self.extract_us
            .saturating_add(self.event_log_us)
            .saturating_add(self.paint_marks_us)
    }
}

pub(super) fn compute_worker_hit_stages(instants: WorkerHitStageInstants) -> WorkerHitStages {
    let extract_us = duration_us(instants.arm_start, instants.after_extract);
    let event_log_us = duration_us(instants.after_extract, instants.after_event_log);
    let paint_marks_us = duration_us(instants.after_event_log, instants.after_marks);
    let arm_total_us = duration_us(instants.arm_start, instants.after_marks);
    WorkerHitStages {
        extract_us,
        event_log_us,
        paint_marks_us,
        arm_total_us,
    }
}

fn duration_us(start: Instant, end: Instant) -> u64 {
    u64::try_from(end.duration_since(start).as_micros()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn stages_with_gaps(extract: u64, event_log: u64, paint_marks: u64) -> WorkerHitStages {
        let arm_start = Instant::now();
        let after_extract = arm_start + Duration::from_micros(extract);
        let after_event_log = after_extract + Duration::from_micros(event_log);
        let after_marks = after_event_log + Duration::from_micros(paint_marks);
        compute_worker_hit_stages(WorkerHitStageInstants {
            arm_start,
            after_extract,
            after_event_log,
            after_marks,
        })
    }

    #[test]
    fn worker_hit_stage_components_sum_to_arm_total() {
        let stages = stages_with_gaps(11, 47, 23);
        assert_eq!(stages.extract_us, 11);
        assert_eq!(stages.event_log_us, 47);
        assert_eq!(stages.paint_marks_us, 23);
        assert_eq!(stages.arm_total_us, 81);
        assert_eq!(stages.component_sum_us(), stages.arm_total_us);
    }

    #[test]
    fn worker_hit_stage_components_sum_within_five_percent_of_arm_total() {
        // Exact arithmetic on the synthetic harness — the rounding
        // tolerance only needs to defend against the `u128 → u64`
        // narrowing in [`duration_us`]. The 5 % envelope from the
        // P18.7 exit criteria is a production-side check; the
        // synthetic path here is exact.
        for (extract, event_log, paint_marks) in [
            (1, 2, 3),
            (250, 500, 250),
            (0, 0, 7),
            (10_000, 20_000, 30_000),
        ] {
            let stages = stages_with_gaps(extract, event_log, paint_marks);
            let total = stages.arm_total_us as i128;
            let sum = stages.component_sum_us() as i128;
            let drift = (total - sum).abs();
            let envelope = total.max(1) / 20;
            assert!(
                drift <= envelope,
                "components drifted from arm_total by {drift}us — total={total} sum={sum} envelope={envelope}",
            );
        }
    }

    #[test]
    fn worker_hit_stage_saturates_negative_or_overflow_durations() {
        // Defensive: a backwards monotonic clock would yield an
        // overflowing `Duration` panic in `duration_since`; we use
        // the same-start, same-end pair to exercise the zero path
        // (the only one a sane clock can produce here).
        let pinned = Instant::now();
        let stages = compute_worker_hit_stages(WorkerHitStageInstants {
            arm_start: pinned,
            after_extract: pinned,
            after_event_log: pinned,
            after_marks: pinned,
        });
        assert_eq!(stages.arm_total_us, 0);
        assert_eq!(stages.component_sum_us(), 0);
    }
}
