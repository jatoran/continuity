//! Map a projection-worker poll outcome onto the [`FrameDisplay`] the
//! paint will use.
//!
//! Extracted from `frame_resolution.rs` so that file stays under the
//! conventions cap. Two paths land here:
//!
//! * `Ok(hit)` — the worker (or the bounded-wait helper) delivered a
//!   stamp-matched [`FrameDisplay`]; emit the `worker_hit` trace pair
//!   and return it as-is. The arm is sub-stage-timed (P18.7) and
//!   emits `event:worker_hit_stages` so the perf analyzer can
//!   distinguish dispatch-arm cost (microseconds) from the upstream
//!   paint preparation that dominates the
//!   `frame_display:worker_hit` paint-mark duration.
//! * `Err(reason)` — fall back to either a cold-deferred stub (when
//!   the classifier said `Cold` and we hold a candidate frame the
//!   stub helper accepts) or an inline realization via
//!   [`Window::realize_projection_build_kind`].
//!
//! Trace label ordering and spellings (`paint:frame_display:worker_hit`,
//! `paint:frame_display:worker_miss`, `paint:frame_display:cold_deferred`,
//! `paint:frame_display:cold_deferred_skip`,
//! `paint:frame_display:scroll_anim_reuse`, `frame_ready source=…`,
//! `event:worker_hit_stages`) are consumed by the perf scripts in
//! `.trash/analyze_trace.py` and by the worker-miss recovery
//! dashboard; they must not drift.
//!
//! Thread ownership: UI thread of one window.

use std::ops::Range;
use std::time::Instant;

use continuity_decorate::Decorations;
use continuity_display_map::{FoldRange, ImageRowReservation};
use continuity_render::FrameDisplay;
use ropey::Rope;

use crate::display_prewarm_cache::PrewarmQuery;
use crate::paint_trace::PaintTrace;
use crate::projection_worker::{ProjectionStamp, StampMismatchField};
use crate::window::Window;
use crate::window_paint::cold_deferred::{cold_deferred_stub_frame, ColdDeferredSkip};
use crate::window_projection_plan::ProjectionBuildKind;
use crate::window_projection_worker::{WorkerMissReason, WorkerOutcome};

use super::live_resize_reuse::{live_resize_reuse_frame, LiveResizeReuseInputs};
use super::scroll_anim_action::ScrollAnimArmInputs;

mod hit_stages;

use hit_stages::{compute_worker_hit_stages, WorkerHitStageInstants};

/// Inputs threaded into
/// [`Window::dispatch_worker_outcome_to_frame_display`].
pub(super) struct WorkerOutcomeDispatchInputs<'a> {
    pub worker_outcome: WorkerOutcome,
    pub viewport_rows: Range<u32>,
    pub projection_kind: &'a ProjectionBuildKind,
    pub rope_for_projection: &'a Rope,
    pub revision_for_projection: u64,
    pub decorations: Option<&'a Decorations>,
    pub caret_bytes_for_projection: &'a [usize],
    pub folds_for_projection: &'a [FoldRange],
    pub image_reservations: &'a [ImageRowReservation],
    pub wrap_width_dip: u32,
    pub projection_char_width: f32,
    pub prior_carets_for_trace: &'a [usize],
    pub selection_reveal_dirty: &'a [u32],
    pub display_query: &'a PrewarmQuery,
    pub scroll_anim_worker_pending: bool,
    /// Fix A — a Ctrl+End / Ctrl+Home / far-reveal jump armed the
    /// off-thread poll and a matching worker build is in flight. Take the
    /// reuse-prior-frame-plus-placeholder path instead of inline-walking
    /// the destination on the UI thread.
    pub jump_offthread_pending: bool,
    pub current_projection_stamp: &'a ProjectionStamp,
}

/// Outputs from [`Window::dispatch_worker_outcome_to_frame_display`].
pub(crate) struct WorkerOutcomeDispatchOutputs {
    pub frame_display: FrameDisplay,
    pub frame_source: &'static str,
    pub worker_miss_reason: Option<&'static str>,
    pub should_skip_cache_seed: bool,
    /// Display-row count the section-10 strip-realize path
    /// synchronously materialized to cover the live viewport. Zero on
    /// every arm except [`ScrollAnimAction::StripRealize`]; surfaced
    /// through to the renderer's `last_scroll_strip_rows` Cell for
    /// the `event:scroll_path` trace.
    pub scroll_strip_rows: u32,
}

/// Whether to reuse the prior frame (covering reuse / strip realize /
/// placeholder) instead of inline-walking the worker-miss build on the
/// UI thread. `reuse_pending` is the caller's gate — true when either a
/// scroll animation or a fix-A jump is in flight with a matching worker
/// request — so a still-building partial worker request and a
/// motion-compatible cached frame let paint defer to the worker. Used for
/// both the scroll-tick strip realize and the Ctrl+End / Ctrl+Home jump
/// off-thread path.
fn should_reuse_prior_frame_for_worker(
    reuse_pending: bool,
    worker_miss_reason: WorkerMissReason,
    is_partial_variant: bool,
    cached_query: Option<&PrewarmQuery>,
    display_query: &PrewarmQuery,
    _reservations_empty: bool,
) -> bool {
    reuse_pending
        && matches!(
            worker_miss_reason,
            WorkerMissReason::NotReady
                | WorkerMissReason::StampMismatch(StampMismatchField::Viewport)
        )
        && is_partial_variant
        && cached_query.is_some_and(|query| query.motion_compat_mismatch(display_query).is_none())
}

impl Window {
    pub(super) fn dispatch_worker_outcome_to_frame_display(
        &mut self,
        inputs: WorkerOutcomeDispatchInputs<'_>,
        trace: &PaintTrace,
    ) -> WorkerOutcomeDispatchOutputs {
        let WorkerOutcomeDispatchInputs {
            worker_outcome,
            viewport_rows,
            projection_kind,
            rope_for_projection,
            revision_for_projection,
            decorations,
            caret_bytes_for_projection,
            folds_for_projection,
            image_reservations,
            wrap_width_dip,
            projection_char_width,
            prior_carets_for_trace,
            selection_reveal_dirty,
            display_query,
            scroll_anim_worker_pending,
            jump_offthread_pending,
            current_projection_stamp,
        } = inputs;
        match worker_outcome.into_result() {
            Ok(hit) => {
                // The off-thread jump build (fix A) landed — stop polling.
                self.jump_offthread_polls = 0;
                // P18.7: sub-stage timing for the worker-hit install
                // arm. The arm itself is microseconds-scoped — the
                // worker frame is `Arc<DisplayMap>` and is moved (not
                // cloned) into the output struct. Instrumentation
                // confirms the arm is cheap so the next perf eval can
                // attribute the `frame_display:worker_hit` paint-mark
                // duration to upstream preparation (bounded wait,
                // selection-reveal diff, classifier) instead of the
                // dispatch arm.
                let trace_enabled = crate::paint_trace::is_trace_enabled();
                let arm_start = trace_enabled.then(Instant::now);

                let worker_seq = hit.seq;
                let build_dur_us = hit.build_dur_us;
                let coalesced_dropped = hit.coalesced_dropped;
                let worker_fd = hit.frame_display;
                let after_extract = trace_enabled.then(Instant::now);

                if trace_enabled {
                    let detail = format!(
                        "seq={worker_seq} viewport={}..{} build_dur_us={build_dur_us} \
                         coalesced_dropped={coalesced_dropped}",
                        viewport_rows.start, viewport_rows.end,
                    );
                    crate::paint_trace::log_event("paint:frame_display:worker_hit", &detail);
                    crate::paint_trace::log_event(
                        "event:projection_worker_result",
                        &format!(
                            "seq={worker_seq} accepted=true build_dur_us={build_dur_us} \
                             coalesced_dropped={coalesced_dropped}"
                        ),
                    );
                }
                let after_event_log = trace_enabled.then(Instant::now);

                trace.mark("frame_display:worker_hit");
                // ε.5g: cumulative time from `WM_PAINT` entry to the
                // moment `FrameDisplay` is in hand. Excludes the
                // renderer's draw submission and the swap-chain
                // `Present` that follow this match. The worker build
                // itself ran on another thread and is not on the UI
                // thread's clock here — this duration is just the
                // UI-thread overhead to accept the result.
                trace.mark_since_start(
                    "frame_ready",
                    &format!("source=worker_hit seq={worker_seq}"),
                );
                let after_marks = trace_enabled.then(Instant::now);

                if let (Some(start), Some(extract), Some(log), Some(marks)) =
                    (arm_start, after_extract, after_event_log, after_marks)
                {
                    let stages = compute_worker_hit_stages(WorkerHitStageInstants {
                        arm_start: start,
                        after_extract: extract,
                        after_event_log: log,
                        after_marks: marks,
                    });
                    crate::paint_trace::log_event(
                        "event:worker_hit_stages",
                        &format!(
                            "extract_us={} event_log_us={} paint_marks_us={} \
                             arm_total_us={} seq={worker_seq}",
                            stages.extract_us,
                            stages.event_log_us,
                            stages.paint_marks_us,
                            stages.arm_total_us,
                        ),
                    );
                }

                WorkerOutcomeDispatchOutputs {
                    frame_display: worker_fd,
                    frame_source: "worker_hit",
                    worker_miss_reason: None,
                    should_skip_cache_seed: false,
                    scroll_strip_rows: 0,
                }
            }
            Err(reason) => {
                let reason_label = reason.as_str();
                if crate::paint_trace::is_trace_enabled() {
                    crate::paint_trace::log_event(
                        "paint:frame_display:worker_miss",
                        &format!("reason={reason_label}"),
                    );
                }
                let reuse_pending = scroll_anim_worker_pending || jump_offthread_pending;
                let reuse_prev_frame = self.last_painted_frame_display.as_ref().and_then(
                    |(cached_query, cached_frame)| {
                        should_reuse_prior_frame_for_worker(
                            reuse_pending,
                            reason,
                            projection_kind.is_partial_variant(),
                            Some(cached_query),
                            display_query,
                            image_reservations.is_empty(),
                        )
                        .then(|| cached_frame.clone())
                    },
                );
                if let Some(prev_frame) = reuse_prev_frame {
                    if jump_offthread_pending {
                        // Cheap placeholder poll for the off-thread jump
                        // (fix A): spend one budget unit and schedule
                        // another paint so the worker's result is taken the
                        // instant it lands. `WM_PAINT` is low priority, so
                        // input preempts this poll — the UI stays responsive
                        // while the destination builds on the worker instead
                        // of freezing on the inline walk.
                        self.jump_offthread_polls = self.jump_offthread_polls.saturating_sub(1);
                        if self.jump_offthread_polls > 0 {
                            self.invalidate_with_reason(self.hwnd(), "jump_offthread_poll");
                        }
                    }
                    return self.run_scroll_anim_arm(
                        ScrollAnimArmInputs {
                            prev_frame,
                            viewport_rows: viewport_rows.clone(),
                            rope_for_projection,
                            revision_for_projection,
                            decorations,
                            caret_bytes_for_projection,
                            folds_for_projection,
                            image_reservations,
                            wrap_width_dip,
                            projection_char_width,
                            reason_label,
                        },
                        trace,
                    );
                }
                // An armed jump that didn't take the reuse path (no
                // motion-compatible frame, non-partial kind, reservations
                // present, …) falls through to the inline build below;
                // stop polling so it doesn't re-arm on later paints.
                self.jump_offthread_polls = 0;
                // γ — these candidates feed both the live-resize reuse
                // and the cold-deferred stub, neither of which inspects
                // the reservation set (they validate against the frame's
                // `IndexStamps`, which carry no reservation signature).
                // So the candidate selection itself must reject a
                // reservation-mismatched frame: filter `last_painted` on
                // the reservation signature and use the reservation-aware
                // spectator lookup. Without this, collapsing an expanded
                // image (reservation set empties while `last_painted`
                // still holds the expanded frame at the same
                // rope/decoration/wrap) would substitute the stale
                // expanded geometry for one frame.
                let raw_last_painted = self
                    .last_painted_frame_display
                    .as_ref()
                    .filter(|(query, _)| {
                        query.document() == display_query.document()
                            && query.image_reservations_signature()
                                == display_query.image_reservations_signature()
                    })
                    .map(|(_, fd)| fd.clone());
                let raw_spectator = self
                    .spectator_frame_cache
                    .borrow()
                    .lookup_same_document_for_reuse(self.tree.focused, display_query);
                let candidate = raw_last_painted.or(raw_spectator);
                let live_resize_reuse = live_resize_reuse_frame(LiveResizeReuseInputs {
                    candidate: candidate.clone(),
                    is_live_resize_shrink_tick: self.is_live_resizing
                        && self.deferred_renderer_resize.is_some(),
                    projection_kind,
                    worker_miss_reason: reason,
                    image_reservations_empty: image_reservations.is_empty(),
                    current_projection_stamp,
                });
                match live_resize_reuse {
                    Ok(prev_frame) => {
                        if crate::paint_trace::is_trace_enabled() {
                            let stamps = prev_frame.row_index().stamps();
                            let realized = prev_frame.realized_row_range();
                            crate::paint_trace::log_event(
                                "paint:frame_display:live_resize_reuse",
                                &format!(
                                    "cached_wrap={} target_wrap={} rope_rev={} reason={} \
                                     realized={}..{} viewport={}..{}",
                                    stamps.wrap_width_dip,
                                    wrap_width_dip,
                                    stamps.rope_revision,
                                    reason_label,
                                    realized.start,
                                    realized.end,
                                    viewport_rows.start,
                                    viewport_rows.end,
                                ),
                            );
                        }
                        trace.mark("frame_display:live_resize_reuse");
                        trace.mark_since_start(
                            "frame_ready",
                            &format!("source=live_resize_reuse reason={reason_label}"),
                        );
                        return WorkerOutcomeDispatchOutputs {
                            frame_display: prev_frame,
                            frame_source: "live_resize_reuse",
                            worker_miss_reason: Some(reason_label),
                            should_skip_cache_seed: true,
                            scroll_strip_rows: 0,
                        };
                    }
                    Err(skip) => {
                        if crate::paint_trace::is_trace_enabled()
                            && self.is_live_resizing
                            && self.deferred_renderer_resize.is_some()
                        {
                            crate::paint_trace::log_event(
                                "paint:frame_display:live_resize_reuse_skip",
                                &format!("reason={} worker_reason={reason_label}", skip.as_str()),
                            );
                        }
                    }
                }
                // Cold-deferred stub: skip the inline row-count walker
                // when the only geometry shift is wrap width and we
                // have a same-rope, same-decoration previous frame.
                // The post-paint dispatch already submits the Cold
                // plan to the projection worker; the next paint will
                // replace the stub with the worker's real frame. The
                // walker dominates Cold builds (~450 ms on a 9 k-line
                // markdown buffer at a new wrap — see
                // `perf-snapshots/manual-lag_after-coalesce_20260518-164726.tsv`
                // events `frame_display:cold_build dur=448572` and
                // `dur=310310`). Raw candidates bypass the upstream
                // motion/hit-test filters, so this call site repeats
                // the helper's wrap-stamp guard before a candidate can
                // substitute for the Cold build. Document match is
                // enforced here so a wrong-buffer frame can never
                // reach the helper.
                let stub = if matches!(projection_kind, ProjectionBuildKind::Cold) {
                    if candidate.as_ref().is_some_and(|frame| {
                        frame.row_index().stamps().wrap_width_dip != wrap_width_dip
                    }) {
                        Some(Err(ColdDeferredSkip::WrapWidthMismatch))
                    } else {
                        Some(cold_deferred_stub_frame(
                            candidate,
                            revision_for_projection,
                            decorations.map(|d| d.revision),
                            wrap_width_dip,
                            rope_for_projection.len_lines(),
                        ))
                    }
                } else {
                    None
                };
                if let Some(Ok(stub_frame)) = stub {
                    if crate::paint_trace::is_trace_enabled() {
                        let stamps = stub_frame.row_index().stamps();
                        crate::paint_trace::log_event(
                            "paint:frame_display:cold_deferred",
                            &format!(
                                "stub_wrap={} target_wrap={} rope_rev={} reason={}",
                                stamps.wrap_width_dip,
                                wrap_width_dip,
                                stamps.rope_revision,
                                reason_label,
                            ),
                        );
                    }
                    trace.mark_since_start(
                        "frame_ready",
                        &format!("source=cold_deferred reason={reason_label}"),
                    );
                    WorkerOutcomeDispatchOutputs {
                        frame_display: stub_frame,
                        frame_source: "cold_deferred",
                        worker_miss_reason: Some(reason_label),
                        should_skip_cache_seed: true,
                        scroll_strip_rows: 0,
                    }
                } else {
                    if let Some(Err(skip)) = stub {
                        if crate::paint_trace::is_trace_enabled() {
                            crate::paint_trace::log_event(
                                "paint:frame_display:cold_deferred_skip",
                                &format!("reason={}", skip.as_str()),
                            );
                        }
                    }
                    let fd = self.realize_projection_build_kind(
                        projection_kind,
                        rope_for_projection,
                        revision_for_projection,
                        decorations,
                        caret_bytes_for_projection,
                        folds_for_projection,
                        image_reservations,
                        wrap_width_dip,
                        projection_char_width,
                        &viewport_rows,
                        prior_carets_for_trace,
                        selection_reveal_dirty,
                        trace,
                    );
                    // ε.5g: cumulative time from `WM_PAINT` entry to
                    // `FrameDisplay`-in-hand via the inline path. This
                    // *does* include the rebuild itself because the
                    // worker missed and the UI thread paid for the
                    // realization — counterpart to the worker_hit
                    // branch above. Detail names the miss reason so
                    // the perf-gate trace tail can break the
                    // distribution out by cause.
                    trace.mark_since_start(
                        "frame_ready",
                        &format!("source=inline_fallback reason={reason_label}"),
                    );
                    let source = match projection_kind.trace_label() {
                        "cache_hit" => "cache_hit",
                        "selection_reveal" => "inline_selection_reveal",
                        "dirty" => "inline_dirty",
                        "splice" => "inline_splice",
                        "viewport_realize" => "inline_viewport_realize",
                        "cold" => "inline_cold",
                        _ => "inline_unknown",
                    };
                    WorkerOutcomeDispatchOutputs {
                        frame_display: fd,
                        frame_source: source,
                        worker_miss_reason: Some(reason_label),
                        should_skip_cache_seed: false,
                        scroll_strip_rows: 0,
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_buffer::BufferId;
    use continuity_display_map::FoldRange;
    use continuity_layout::FontStateId;

    fn scroll_query(buffer_id: BufferId, revision: u64) -> PrewarmQuery {
        PrewarmQuery::new(
            buffer_id,
            revision,
            Some(revision),
            &[0],
            &[] as &[FoldRange],
            480,
            FontStateId::from_parts("Cascadia Mono", 14.0, "en-us", 1.0),
        )
    }

    #[test]
    fn reuses_compatible_partial_frame_instead_of_inline_cold_partial() {
        let buffer_id = BufferId::new();
        let query = scroll_query(buffer_id, 7);

        // `reuse_pending` true (scroll-anim or jump) + NotReady + partial +
        // motion-compatible cached frame → reuse the prior frame.
        assert!(should_reuse_prior_frame_for_worker(
            true,
            WorkerMissReason::NotReady,
            true,
            Some(&query),
            &query,
            true,
        ));
    }

    #[test]
    fn reuse_rejected_when_not_pending() {
        let buffer_id = BufferId::new();
        let query = scroll_query(buffer_id, 7);

        // No scroll-anim and no armed jump → never reuse; paint resolves
        // through the normal cold/dirty/inline path.
        assert!(!should_reuse_prior_frame_for_worker(
            false,
            WorkerMissReason::NotReady,
            true,
            Some(&query),
            &query,
            true,
        ));
    }

    #[test]
    fn reuse_rejects_revision_drift() {
        let buffer_id = BufferId::new();
        let cached = scroll_query(buffer_id, 7);
        let current = scroll_query(buffer_id, 8);

        assert!(!should_reuse_prior_frame_for_worker(
            true,
            WorkerMissReason::NotReady,
            true,
            Some(&cached),
            &current,
            true,
        ));
    }
}
