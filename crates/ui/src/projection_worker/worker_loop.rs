// ε.5 ships the worker foundation only; until the integration slice
// wires `Window::on_paint` to dispatch + validate worker results,
// these helpers read "never used".
#![allow(dead_code)]
//! Worker-thread internals: the receive/coalesce/build/publish loop and
//! the per-request projection builder.
//!
//! [`worker_loop`] is the sole reader of the request channel and the
//! sole writer of the result queue — the single-writer rule.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use continuity_display_map::wrap::WidthMeasure;
use continuity_display_map::{SegmentCache, WrapCache};
use continuity_render::FrameDisplay;
use crossbeam_channel::{Receiver, TryRecvError};

use super::measure::MeasureMode;
use super::schema::{ProjectionPlan, ProjectionRequest, ProjectionResult};
use super::{
    push_bounded_result, PendingProjectionRequests, ResultCell, PAINT_PARTIAL_FILL_REASON,
};

pub(super) fn worker_loop(
    cmd_rx: Receiver<ProjectionRequest>,
    measure_mode: MeasureMode,
    latest_result: Arc<ResultCell>,
    pending_requests: PendingProjectionRequests,
    wrap_cache: Arc<WrapCache>,
    segment_cache: Arc<SegmentCache>,
    processed_count: Arc<AtomicU64>,
) {
    loop {
        // Block until at least one request arrives. Err = all senders
        // dropped (shutdown).
        let first = match cmd_rx.recv() {
            Ok(req) => req,
            Err(_) => return,
        };
        // Drain newer requests; keep the most recent request per target
        // pane, plus a same-revision partial-fill request that would
        // otherwise be starved by paint churn. Disconnect mid-drain
        // still produces the retained batch then exits.
        // `coalesced_dropped` counts older requests for the same target
        // pane that this build replaces.
        let mut disconnected = false;
        let mut batch = vec![first];
        loop {
            match cmd_rx.try_recv() {
                Ok(req) => batch.push(req),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
        let retained = retain_latest_request_per_target(batch, &pending_requests);
        for retained_request in retained {
            let started = Instant::now();
            let mut result = build_for_request(
                &retained_request.request,
                &measure_mode,
                &wrap_cache,
                &segment_cache,
            );
            result.build_dur_us = started.elapsed().as_micros().min(u64::MAX as u128) as u64;
            result.coalesced_dropped = retained_request.coalesced_dropped;
            remove_pending_request(&pending_requests, retained_request.request.seq);
            processed_count.fetch_add(1, Ordering::Relaxed);
            if let Ok(mut cell) = latest_result.results.lock() {
                push_bounded_result(&mut cell, result);
                // Wake any paint thread currently waiting in
                // `ProjectionWorker::wait_for_result_publication`.
                // notify_all (not notify_one) because there is only
                // ever one expected waiter and over-notifying is free.
                latest_result.publication.notify_all();
            }
        }
        if disconnected {
            return;
        }
    }
}

fn remove_pending_request(pending_requests: &PendingProjectionRequests, seq: u64) {
    if let Ok(mut pending) = pending_requests.lock() {
        pending.retain(|request| request.seq != seq);
    }
}

struct RetainedRequest {
    request: ProjectionRequest,
    coalesced_dropped: u32,
}

fn retain_latest_request_per_target(
    batch: Vec<ProjectionRequest>,
    pending_requests: &PendingProjectionRequests,
) -> Vec<RetainedRequest> {
    let mut retained: Vec<RetainedRequest> = Vec::new();
    for request in batch.into_iter().rev() {
        let is_partial_fill = is_pending_partial_fill(pending_requests, request.seq);
        if let Some(existing_idx) = retained
            .iter()
            .position(|entry| entry.request.target_pane == request.target_pane)
        {
            let newer_is_partial_fill =
                is_pending_partial_fill(pending_requests, retained[existing_idx].request.seq);
            if should_retain_partial_fill(
                &request,
                is_partial_fill,
                newer_is_partial_fill,
                &retained[existing_idx].request,
            ) {
                retained.push(RetainedRequest {
                    request,
                    coalesced_dropped: 0,
                });
            } else {
                let existing = &mut retained[existing_idx];
                existing.coalesced_dropped = existing.coalesced_dropped.saturating_add(1);
                remove_pending_request(pending_requests, request.seq);
            }
        } else {
            retained.push(RetainedRequest {
                request,
                coalesced_dropped: 0,
            });
        }
    }
    retained.reverse();
    retained
}

fn is_pending_partial_fill(pending_requests: &PendingProjectionRequests, seq: u64) -> bool {
    pending_requests.lock().ok().is_some_and(|pending| {
        pending
            .iter()
            .any(|entry| entry.seq == seq && entry.reason == PAINT_PARTIAL_FILL_REASON)
    })
}

fn should_retain_partial_fill(
    request: &ProjectionRequest,
    is_partial_fill: bool,
    newer_is_partial_fill: bool,
    newer_request: &ProjectionRequest,
) -> bool {
    is_partial_fill
        && !newer_is_partial_fill
        && request.stamp.document == newer_request.stamp.document
        && request.stamp.rope_revision == newer_request.stamp.rope_revision
        && request.stamp.font_state == newer_request.stamp.font_state
        && request.stamp.wrap_width_dip == newer_request.stamp.wrap_width_dip
}

fn build_for_request(
    req: &ProjectionRequest,
    measure_mode: &MeasureMode,
    wrap_cache: &WrapCache,
    segment_cache: &SegmentCache,
) -> ProjectionResult {
    let mut measure = measure_mode.build_measure(
        &req.font_metrics,
        req.fallback_char_width_dip,
        req.stamp.font_state,
    );
    let measure_ref: &mut dyn WidthMeasure = &mut *measure;
    let decorations = req.decorations.as_deref();
    let frame_display = match &req.plan {
        ProjectionPlan::Cold => FrameDisplay::build_viewport_measured_with_caches(
            &req.rope,
            req.stamp.rope_revision,
            decorations,
            &req.caret_bytes,
            &req.folds,
            &req.image_reservations,
            &req.suppressed_table_blocks,
            req.markdown_toggles,
            req.stamp.wrap_width_dip,
            measure_ref,
            req.stamp.viewport_rows.clone(),
            req.stamp.overscan,
            req.stamp.font_state.0,
            crate::window::FONT_LOCALE,
            wrap_cache,
            segment_cache,
        ),
        ProjectionPlan::Dirty { prev, dirty } => FrameDisplay::rebuild_dirty_measured(
            prev,
            dirty,
            &req.rope,
            req.stamp.rope_revision,
            decorations,
            &req.caret_bytes,
            &req.folds,
            &req.image_reservations,
            &req.suppressed_table_blocks,
            req.markdown_toggles,
            req.stamp.wrap_width_dip,
            measure_ref,
            req.stamp.viewport_rows.clone(),
            req.stamp.overscan,
        ),
        ProjectionPlan::Splice { prev, splice } => FrameDisplay::rebuild_spliced_measured(
            prev,
            splice,
            &req.rope,
            req.stamp.rope_revision,
            decorations,
            &req.caret_bytes,
            &req.folds,
            &req.image_reservations,
            &req.suppressed_table_blocks,
            req.markdown_toggles,
            req.stamp.wrap_width_dip,
            measure_ref,
            req.stamp.viewport_rows.clone(),
            req.stamp.overscan,
        ),
    };
    ProjectionResult {
        seq: req.seq,
        target_pane: req.target_pane,
        stamp: req.stamp.clone(),
        frame_display,
        // Filled in by `worker_loop` after the build returns; the
        // builder itself doesn't see the wall clock.
        build_dur_us: 0,
        coalesced_dropped: 0,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use continuity_display_map::{FoldRange, ImageRowReservation};
    use continuity_layout::FontStateId;
    use ropey::Rope;

    use super::*;
    use crate::pane_tree::PaneId;
    use crate::projection_worker::{PendingProjectionRequest, ProjectionStamp, WorkerFontMetrics};

    fn request(seq: u64, target_pane: PaneId, revision: u64) -> ProjectionRequest {
        let caret_bytes: Arc<[usize]> = Arc::from(vec![0]);
        let folds: Arc<[FoldRange]> = Arc::from(Vec::<FoldRange>::new());
        let reservations: Arc<[ImageRowReservation]> = Arc::from(Vec::<ImageRowReservation>::new());
        ProjectionRequest {
            seq,
            target_pane,
            stamp: ProjectionStamp {
                document: 11,
                rope_revision: revision,
                decoration_revision: None,
                decoration_parse_revision: None,
                caret_signature: ProjectionStamp::caret_signature(&caret_bytes),
                fold_signature: ProjectionStamp::fold_signature(&folds),
                image_reservations_signature: ProjectionStamp::image_reservations_signature(
                    &reservations,
                ),
                wrap_width_dip: 480,
                font_state: FontStateId::default(),
                viewport_rows: 0..8,
                overscan: 20,
            },
            rope: Arc::new(Rope::from_str("a\nb\n")),
            decorations: None,
            caret_bytes,
            folds,
            image_reservations: reservations,
            suppressed_table_blocks: Arc::from(Vec::new()),
            markdown_toggles: continuity_display_map::MarkdownRenderToggles::default(),
            fallback_char_width_dip: 8.0,
            font_metrics: WorkerFontMetrics::fallback(8.0),
            plan: ProjectionPlan::Cold,
        }
    }

    fn pending_with_reason(
        seq: u64,
        request: &ProjectionRequest,
        reason: &'static str,
    ) -> PendingProjectionRequest {
        PendingProjectionRequest {
            seq,
            target_pane: request.target_pane,
            stamp: request.stamp.clone(),
            reason,
        }
    }

    #[test]
    fn coalescing_retains_same_revision_partial_fill_with_latest_same_pane_request() {
        let pane = PaneId::fresh();
        let fill = request(1, pane, 7);
        let churn = request(2, pane, 7);
        let pending = Arc::new(Mutex::new(VecDeque::from(vec![
            pending_with_reason(1, &fill, PAINT_PARTIAL_FILL_REASON),
            pending_with_reason(2, &churn, "paint_epilogue"),
        ])));

        let retained = retain_latest_request_per_target(vec![fill, churn], &pending);

        let seqs: Vec<u64> = retained.iter().map(|entry| entry.request.seq).collect();
        assert_eq!(seqs, vec![1, 2]);
    }

    #[test]
    fn coalescing_drops_stale_partial_fill_when_newer_revision_is_queued() {
        let pane = PaneId::fresh();
        let stale_fill = request(1, pane, 7);
        let newer = request(2, pane, 8);
        let pending = Arc::new(Mutex::new(VecDeque::from(vec![
            pending_with_reason(1, &stale_fill, PAINT_PARTIAL_FILL_REASON),
            pending_with_reason(2, &newer, "paint_epilogue"),
        ])));

        let retained = retain_latest_request_per_target(vec![stale_fill, newer], &pending);

        let seqs: Vec<u64> = retained.iter().map(|entry| entry.request.seq).collect();
        assert_eq!(seqs, vec![2]);
    }

    #[test]
    fn coalescing_keeps_only_latest_partial_fill_for_same_pane() {
        let pane = PaneId::fresh();
        let older_fill = request(1, pane, 7);
        let newer_fill = request(2, pane, 7);
        let pending = Arc::new(Mutex::new(VecDeque::from(vec![
            pending_with_reason(1, &older_fill, PAINT_PARTIAL_FILL_REASON),
            pending_with_reason(2, &newer_fill, PAINT_PARTIAL_FILL_REASON),
        ])));

        let retained = retain_latest_request_per_target(vec![older_fill, newer_fill], &pending);

        let seqs: Vec<u64> = retained.iter().map(|entry| entry.request.seq).collect();
        assert_eq!(seqs, vec![2]);
    }
}
