//! Per-paint worker-result acceptance.
//!
//! [`try_use_worker_result`] is the gate that decides whether the
//! current paint can replay a precomputed worker [`FrameDisplay`] or
//! must fall through to a UI-thread build. The target-pane result queue
//! entry is drained as part of the check — a stamp-mismatched result
//! is still consumed because that result was for a prior paint's inputs
//! and can never become valid.

use continuity_display_map::ImageRowReservation;
use continuity_render::FrameDisplay;

use super::miss_reason::WorkerMissReason;
use crate::pane_tree::PaneId;
use crate::projection_worker::{ProjectionStamp, ProjectionWorker};

/// One accepted worker result. Carries the seq and the latency
/// breakdown so callers can attach `build_dur_us` and
/// `coalesced_dropped` to the `paint:frame_display:worker_hit` trace.
pub(crate) struct WorkerHit {
    pub frame_display: FrameDisplay,
    pub seq: u64,
    pub build_dur_us: u64,
    pub coalesced_dropped: u32,
}

/// A worker result whose stamp drifted past the current paint. The
/// stamp itself rides back so the trace can attribute *how far* the
/// worker was behind — e.g. the rope advanced by N revisions while
/// the worker was mid-build. The caller swaps this into the
/// `paint:frame_display:worker_miss reason=… stale_*=…` event.
pub(crate) struct WorkerStaleResult {
    pub seq: u64,
    pub stamp: ProjectionStamp,
    pub build_dur_us: u64,
    pub coalesced_dropped: u32,
}

/// Outcome of trying to consume a worker result.
pub(crate) enum WorkerOutcome {
    Hit(WorkerHit),
    /// The worker produced a result but the stamp drifted past the
    /// current paint's. The mismatch field plus the stale stamp
    /// itself feed the miss trace.
    StaleResult {
        field: crate::projection_worker::StampMismatchField,
        stale: WorkerStaleResult,
    },
    /// The worker result belongs to a different buffer than the
    /// focused paint. It is logged as a stale delivery but converted
    /// to the same miss as an empty result mailbox.
    CrossBufferResult {
        stale: WorkerStaleResult,
    },
    /// No worker result was available (no worker or channel empty).
    NoResult(WorkerMissReason),
}

impl WorkerOutcome {
    /// Map back to the legacy `Result` shape the orchestrator expects.
    /// Carries enough info on miss for the trace event but throws the
    /// stale-stamp breadcrumb away for the orchestrator's value.
    pub(crate) fn into_result(self) -> Result<WorkerHit, WorkerMissReason> {
        match self {
            Self::Hit(hit) => Ok(hit),
            Self::StaleResult { field, .. } => Err(WorkerMissReason::StampMismatch(field)),
            Self::CrossBufferResult { .. } => Err(WorkerMissReason::NotReady),
            Self::NoResult(reason) => Err(reason),
        }
    }
}

/// Try to consume a worker result for the current paint.
///
/// Returns the rich [`WorkerOutcome`] so the orchestrator can attach
/// `build_dur_us` / `coalesced_dropped` to the worker_hit trace and
/// `stale_seq` / `stale_rope_rev` / `stale_build_dur_us` to the
/// worker_miss trace.
pub(crate) fn try_use_worker_result_rich(
    worker: Option<&ProjectionWorker>,
    target_pane: PaneId,
    current_stamp: &ProjectionStamp,
    _image_reservations: &[ImageRowReservation],
    is_first_paint: bool,
) -> WorkerOutcome {
    let Some(worker) = worker else {
        return WorkerOutcome::NoResult(if is_first_paint {
            WorkerMissReason::FirstPaint
        } else {
            WorkerMissReason::WorkerAbsent
        });
    };
    let Some(result) = worker.take_latest_result_for_target(target_pane) else {
        return WorkerOutcome::NoResult(if is_first_paint {
            WorkerMissReason::FirstPaint
        } else {
            WorkerMissReason::NotReady
        });
    };
    if result.stamp.document != current_stamp.document {
        return WorkerOutcome::CrossBufferResult {
            stale: WorkerStaleResult {
                seq: result.seq,
                stamp: result.stamp.clone(),
                build_dur_us: result.build_dur_us,
                coalesced_dropped: result.coalesced_dropped,
            },
        };
    }
    if let Some(field) = result.stamp.diff_field(current_stamp) {
        return WorkerOutcome::StaleResult {
            field,
            stale: WorkerStaleResult {
                seq: result.seq,
                stamp: result.stamp.clone(),
                build_dur_us: result.build_dur_us,
                coalesced_dropped: result.coalesced_dropped,
            },
        };
    }
    WorkerOutcome::Hit(WorkerHit {
        frame_display: result.frame_display,
        seq: result.seq,
        build_dur_us: result.build_dur_us,
        coalesced_dropped: result.coalesced_dropped,
    })
}

/// Legacy wrapper that throws away the latency breadcrumb. Retained
/// for unit tests that still match on the simple `Result` shape.
#[cfg(test)]
pub(crate) fn try_use_worker_result(
    worker: Option<&ProjectionWorker>,
    target_pane: PaneId,
    current_stamp: &ProjectionStamp,
    image_reservations: &[ImageRowReservation],
    is_first_paint: bool,
) -> Result<(FrameDisplay, u64), WorkerMissReason> {
    match try_use_worker_result_rich(
        worker,
        target_pane,
        current_stamp,
        image_reservations,
        is_first_paint,
    )
    .into_result()
    {
        Ok(hit) => Ok((hit.frame_display, hit.seq)),
        Err(reason) => Err(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use continuity_buffer::BufferId;
    use continuity_display_map::{FoldRange, ImageRowReservation, MarkdownRenderToggles};

    use super::super::request_build::build_projection_request;
    use super::super::stamp::{current_projection_stamp, test_inputs};
    use crate::projection_worker::StampMismatchField;

    #[test]
    fn try_use_returns_worker_absent_without_worker() {
        let buf = BufferId::new();
        let folds: Vec<FoldRange> = Vec::new();
        let reservations: Vec<ImageRowReservation> = Vec::new();
        let stamp = current_projection_stamp(&test_inputs(
            buf,
            1,
            None,
            &[0],
            &folds,
            &reservations,
            0..40,
        ));
        let outcome = try_use_worker_result(
            None,
            PaneId::fresh(),
            &stamp,
            &reservations,
            /* first_paint = */ false,
        );
        assert_eq!(outcome.err(), Some(WorkerMissReason::WorkerAbsent));
    }

    #[test]
    fn try_use_returns_first_paint_without_worker_on_first_paint() {
        let buf = BufferId::new();
        let folds: Vec<FoldRange> = Vec::new();
        let reservations: Vec<ImageRowReservation> = Vec::new();
        let stamp = current_projection_stamp(&test_inputs(
            buf,
            1,
            None,
            &[0],
            &folds,
            &reservations,
            0..40,
        ));
        let outcome = try_use_worker_result(
            None,
            PaneId::fresh(),
            &stamp,
            &reservations,
            /* first_paint = */ true,
        );
        assert_eq!(outcome.err(), Some(WorkerMissReason::FirstPaint));
    }

    #[test]
    fn try_use_does_not_block_image_reservations_without_worker() {
        let buf = BufferId::new();
        let folds: Vec<FoldRange> = Vec::new();
        let reservations = vec![ImageRowReservation {
            source_line: continuity_display_map::SourceLine(2),
            reserved_display_rows: 3,
        }];
        let stamp = current_projection_stamp(&test_inputs(
            buf,
            1,
            None,
            &[0],
            &folds,
            &reservations,
            0..40,
        ));
        let outcome = try_use_worker_result(None, PaneId::fresh(), &stamp, &reservations, false);
        assert_eq!(outcome.err(), Some(WorkerMissReason::WorkerAbsent));
    }

    #[test]
    fn try_use_returns_not_ready_when_worker_has_no_result() {
        let buf = BufferId::new();
        let folds: Vec<FoldRange> = Vec::new();
        let reservations: Vec<ImageRowReservation> = Vec::new();
        let stamp = current_projection_stamp(&test_inputs(
            buf,
            1,
            None,
            &[0],
            &folds,
            &reservations,
            0..40,
        ));
        let worker = ProjectionWorker::spawn(crate::projection_worker::MeasureMode::Fixed);
        let outcome =
            try_use_worker_result(Some(&worker), PaneId::fresh(), &stamp, &reservations, false);
        assert_eq!(outcome.err(), Some(WorkerMissReason::NotReady));
    }

    #[test]
    fn try_use_accepts_matching_stamp_and_rejects_mismatched() {
        use crate::projection_worker::{MeasureMode, ProjectionPlan, WorkerFontMetrics};
        use ropey::Rope;
        use std::time::{Duration, Instant};

        let buf = BufferId::new();
        let folds: Vec<FoldRange> = Vec::new();
        let reservations: Vec<ImageRowReservation> = Vec::new();
        let stamp = current_projection_stamp(&test_inputs(
            buf,
            1,
            None,
            &[0],
            &folds,
            &reservations,
            0..8,
        ));
        let target_pane = PaneId::fresh();

        let worker = ProjectionWorker::spawn(MeasureMode::Fixed);
        let rope = Rope::from_str("a\nb\nc\nd\ne\nf\ng\nh\n");
        let request = build_projection_request(
            1,
            target_pane,
            stamp.clone(),
            &rope,
            None,
            &[0],
            &folds,
            &reservations,
            &[],
            MarkdownRenderToggles::default(),
            8.0,
            WorkerFontMetrics::fallback(8.0),
            ProjectionPlan::Cold,
        );
        assert!(worker.submit(request));

        // Spin until the worker writes a result.
        let start = Instant::now();
        let hit = loop {
            if let Ok(hit) =
                try_use_worker_result(Some(&worker), target_pane, &stamp, &reservations, false)
            {
                break hit;
            }
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "worker should produce a result"
            );
            std::thread::sleep(Duration::from_millis(2));
        };
        assert_eq!(hit.1, 1, "worker result seq matches dispatched request seq");

        // Subsequent take is NotReady (cell drained).
        let again = try_use_worker_result(Some(&worker), target_pane, &stamp, &reservations, false);
        assert_eq!(again.err(), Some(WorkerMissReason::NotReady));

        // Submit a request with a fresh stamp (caret moved), then ask
        // for the worker's result against the ORIGINAL stamp. The
        // worker built against the new inputs; old stamp must reject.
        let new_stamp = current_projection_stamp(&test_inputs(
            buf,
            1,
            None,
            &[42],
            &folds,
            &reservations,
            0..8,
        ));
        let new_request = build_projection_request(
            2,
            target_pane,
            new_stamp,
            &rope,
            None,
            &[42],
            &folds,
            &reservations,
            &[],
            MarkdownRenderToggles::default(),
            8.0,
            WorkerFontMetrics::fallback(8.0),
            ProjectionPlan::Cold,
        );
        assert!(worker.submit(new_request));
        let start = Instant::now();
        loop {
            let outcome =
                try_use_worker_result(Some(&worker), target_pane, &stamp, &reservations, false);
            if matches!(
                outcome.err(),
                Some(WorkerMissReason::StampMismatch(
                    StampMismatchField::CaretSignature
                ))
            ) {
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "worker should produce a result with the new stamp"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Field-level miss-reason coverage. A unit-level fake stamp is
    /// injected into the result cell so we don't depend on a real
    /// worker build for the field-naming assertion.
    fn inject_result_with_stamp(
        worker: &ProjectionWorker,
        target_pane: PaneId,
        stamp: ProjectionStamp,
    ) {
        use crate::projection_worker::ProjectionResult;
        use ropey::Rope;
        // Build a tiny FrameDisplay so ProjectionResult is well-formed.
        let mut measure = continuity_display_map::wrap::FixedCharWidth::new(8.0);
        let rope = Rope::from_str("a\n");
        let frame_display = FrameDisplay::build_viewport_measured(
            &rope,
            stamp.rope_revision,
            None,
            &[0usize],
            &[],
            &[],
            0,
            &mut measure,
            0..1,
            0,
        );
        worker.inject_result_for_test(ProjectionResult {
            seq: 0,
            target_pane,
            stamp,
            frame_display,
            build_dur_us: 0,
            coalesced_dropped: 0,
        });
    }

    fn drain_until_field(
        worker: &ProjectionWorker,
        request_stamp: ProjectionStamp,
        check_stamp: &ProjectionStamp,
    ) -> WorkerMissReason {
        let target_pane = PaneId::fresh();
        inject_result_with_stamp(worker, target_pane, request_stamp);
        try_use_worker_result(
            Some(worker),
            target_pane,
            check_stamp,
            &Vec::<ImageRowReservation>::new(),
            false,
        )
        .err()
        .expect("stamp diff should produce a miss")
    }

    #[test]
    fn stamp_mismatch_field_named_for_rope_revision_drift() {
        let buf = BufferId::new();
        let folds: Vec<FoldRange> = Vec::new();
        let reservations: Vec<ImageRowReservation> = Vec::new();
        let mut request_stamp = current_projection_stamp(&test_inputs(
            buf,
            1,
            None,
            &[0],
            &folds,
            &reservations,
            0..1,
        ));
        let check_stamp = current_projection_stamp(&test_inputs(
            buf,
            2,
            None,
            &[0],
            &folds,
            &reservations,
            0..1,
        ));
        // Worker built at rev 1, paint at rev 2.
        request_stamp.rope_revision = 1;
        let worker = ProjectionWorker::spawn(crate::projection_worker::MeasureMode::Fixed);
        let reason = drain_until_field(&worker, request_stamp, &check_stamp);
        assert_eq!(
            reason,
            WorkerMissReason::StampMismatch(StampMismatchField::RopeRevision),
        );
        assert_eq!(reason.as_str(), "stamp_mismatch_rope_revision");
    }

    #[test]
    fn stamp_mismatch_field_named_for_caret_drift() {
        let buf = BufferId::new();
        let folds: Vec<FoldRange> = Vec::new();
        let reservations: Vec<ImageRowReservation> = Vec::new();
        let request_stamp = current_projection_stamp(&test_inputs(
            buf,
            1,
            None,
            &[0],
            &folds,
            &reservations,
            0..1,
        ));
        let check_stamp = current_projection_stamp(&test_inputs(
            buf,
            1,
            None,
            &[5],
            &folds,
            &reservations,
            0..1,
        ));
        let worker = ProjectionWorker::spawn(crate::projection_worker::MeasureMode::Fixed);
        let reason = drain_until_field(&worker, request_stamp, &check_stamp);
        assert_eq!(
            reason,
            WorkerMissReason::StampMismatch(StampMismatchField::CaretSignature),
        );
        assert_eq!(reason.as_str(), "stamp_mismatch_caret_signature");
    }

    #[test]
    fn stamp_mismatch_field_named_for_viewport_drift() {
        let buf = BufferId::new();
        let folds: Vec<FoldRange> = Vec::new();
        let reservations: Vec<ImageRowReservation> = Vec::new();
        let request_stamp = current_projection_stamp(&test_inputs(
            buf,
            1,
            None,
            &[0],
            &folds,
            &reservations,
            0..1,
        ));
        let check_stamp = current_projection_stamp(&test_inputs(
            buf,
            1,
            None,
            &[0],
            &folds,
            &reservations,
            5..6,
        ));
        let worker = ProjectionWorker::spawn(crate::projection_worker::MeasureMode::Fixed);
        let reason = drain_until_field(&worker, request_stamp, &check_stamp);
        assert_eq!(
            reason,
            WorkerMissReason::StampMismatch(StampMismatchField::Viewport),
        );
        assert_eq!(reason.as_str(), "stamp_mismatch_viewport");
    }

    #[test]
    fn stamp_mismatch_field_named_for_decoration_drift() {
        let buf = BufferId::new();
        let folds: Vec<FoldRange> = Vec::new();
        let reservations: Vec<ImageRowReservation> = Vec::new();
        let request_stamp = current_projection_stamp(&test_inputs(
            buf,
            1,
            None,
            &[0],
            &folds,
            &reservations,
            0..1,
        ));
        let check_stamp = current_projection_stamp(&test_inputs(
            buf,
            1,
            Some(9),
            &[0],
            &folds,
            &reservations,
            0..1,
        ));
        let worker = ProjectionWorker::spawn(crate::projection_worker::MeasureMode::Fixed);
        let reason = drain_until_field(&worker, request_stamp, &check_stamp);
        assert_eq!(
            reason,
            WorkerMissReason::StampMismatch(StampMismatchField::DecorationRevision),
        );
        assert_eq!(reason.as_str(), "stamp_mismatch_decoration_revision");
    }
}
