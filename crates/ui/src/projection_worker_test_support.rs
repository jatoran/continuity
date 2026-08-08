//! Probe helpers used by projection-worker integration tests.

use std::time::{Duration, Instant};

use continuity_buffer::BufferId;
use continuity_display_map::{FoldRange, ImageRowReservation, MarkdownRenderToggles};
use continuity_layout::FontStateId;
use ropey::Rope;

use crate::pane_tree::PaneId;
use crate::projection_worker::{
    MeasureMode, ProjectionPlan, ProjectionStamp, ProjectionWorker, WorkerFontMetrics,
};
use crate::window_projection_worker::{
    build_projection_request, try_use_worker_result_rich, WorkerOutcome,
};

/// Observed outcome from feeding a worker result into paint acceptance.
pub struct WorkerAcceptanceProbe {
    /// Worker miss reason after the rich outcome is converted.
    pub miss_reason: &'static str,
    /// Stale field recorded for the rejected result.
    pub stale_field: Option<&'static str>,
    /// Buffer id carried by the worker result.
    pub result_buffer_id: u128,
    /// Buffer id carried by the focused paint stamp.
    pub paint_buffer_id: u128,
}

/// Check that a cross-buffer delivery converts to the no-result miss.
#[must_use]
pub fn probe_cross_buffer_delivery() -> Option<WorkerAcceptanceProbe> {
    let result_buffer_id = BufferId::new().as_uuid().as_u128();
    let paint_buffer_id = BufferId::new().as_uuid().as_u128();
    let result_stamp = test_stamp(result_buffer_id);
    let paint_stamp = test_stamp(paint_buffer_id);
    let worker = ProjectionWorker::spawn(MeasureMode::Fixed);
    let target_pane = PaneId::fresh();
    let rope = Rope::from_str("alpha\nbeta\n");
    let folds: [FoldRange; 0] = [];
    let reservations: [ImageRowReservation; 0] = [];
    let request = build_projection_request(
        1,
        target_pane,
        result_stamp,
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
    if !worker.submit(request) {
        return None;
    }
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        let outcome = try_use_worker_result_rich(
            Some(&worker),
            target_pane,
            &paint_stamp,
            &reservations,
            false,
        );
        match outcome {
            WorkerOutcome::NoResult(_) => std::thread::sleep(Duration::from_millis(1)),
            _ => {
                return Some(probe_from_outcome(
                    outcome,
                    result_buffer_id,
                    paint_buffer_id,
                ));
            }
        }
    }
    None
}

/// Check that a matching pending stamp is visible to paint-epilogue dedupe.
#[must_use]
pub fn probe_pending_stamp_dedupe() -> bool {
    let stamp = test_stamp(BufferId::new().as_uuid().as_u128());
    let worker = ProjectionWorker::spawn(MeasureMode::Fixed);
    worker.record_pending_request_for_probe(7, stamp.clone());
    worker.has_pending_stamp(&stamp)
}

/// Check the completed-frame-source skip policy used by paint epilogue.
#[must_use]
pub fn probe_served_frame_source_dedupe() -> (bool, bool, bool) {
    (
        crate::window_paint::should_skip_projection_worker_request_for_frame_source("cache_hit"),
        crate::window_paint::should_skip_projection_worker_request_for_frame_source("worker_hit"),
        crate::window_paint::should_skip_projection_worker_request_for_frame_source("inline_dirty"),
    )
}

fn probe_from_outcome(
    outcome: WorkerOutcome,
    result_buffer_id: u128,
    paint_buffer_id: u128,
) -> WorkerAcceptanceProbe {
    let stale_field = match &outcome {
        WorkerOutcome::StaleResult { field, .. } => Some(field.as_str()),
        WorkerOutcome::CrossBufferResult { .. } => Some("document"),
        WorkerOutcome::Hit(_) | WorkerOutcome::NoResult(_) => None,
    };
    let miss_reason = outcome
        .into_result()
        .err()
        .map_or("hit", |reason| reason.as_str());
    WorkerAcceptanceProbe {
        miss_reason,
        stale_field,
        result_buffer_id,
        paint_buffer_id,
    }
}

fn test_stamp(document: u128) -> ProjectionStamp {
    let caret_bytes = [0usize];
    let folds: [FoldRange; 0] = [];
    let reservations: [ImageRowReservation; 0] = [];
    ProjectionStamp {
        document,
        rope_revision: 1,
        decoration_revision: None,
        decoration_parse_revision: None,
        caret_signature: ProjectionStamp::caret_signature(&caret_bytes),
        fold_signature: ProjectionStamp::fold_signature(&folds),
        image_reservations_signature: ProjectionStamp::image_reservations_signature(&reservations),
        wrap_width_dip: 0,
        font_state: FontStateId::default(),
        viewport_rows: 0..2,
        overscan: 0,
    }
}
