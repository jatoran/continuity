//! Section-10 scroll-tick strip-realize decision helpers + the
//! dispatcher arm that drives them.
//!
//! Extracted from `worker_outcome_dispatch.rs` to keep that file under
//! the conventions cap. The pure heuristic
//! ([`decide_scroll_anim_action`]) and the dispatcher arm
//! ([`Window::run_scroll_anim_arm`]) live together because they share
//! the trace-event spellings and the rebuild-budget invariant.
//!
//! Thread ownership: UI thread of one window.

use std::time::Instant;

use continuity_decorate::Decorations;
use continuity_display_map::{FoldRange, ImageRowReservation};
use continuity_render::FrameDisplay;
use ropey::Rope;

use crate::paint_trace::PaintTrace;
use crate::window::Window;

use super::worker_outcome_dispatch::WorkerOutcomeDispatchOutputs;

/// Maximum number of display rows the dispatch synchronously realizes
/// on a scroll-tick paint when the cached frame's realized window
/// does not cover the live viewport. Each row is roughly one spec
/// materialization plus index updates — sub-millisecond on warm
/// segment / wrap / layout caches at this cap. Larger gaps mean the
/// inertia out-scrolled the prewarm; we fall through to a placeholder
/// strip and let the worker catch up in the background.
pub(crate) const SCROLL_ANIM_STRIP_REALIZE_MAX_ROWS: u32 = 80;

/// Count display rows in `visible` that fall outside `realized`. The
/// scroll-tick strip realize uses this to decide between extending
/// the cached frame inline (small gap) and falling back to a
/// placeholder strip (large gap).
#[must_use]
pub(crate) fn uncovered_row_count(
    realized: std::ops::Range<u32>,
    visible: std::ops::Range<u32>,
) -> u32 {
    if visible.start >= visible.end {
        return 0;
    }
    let top_gap = realized.start.saturating_sub(visible.start);
    let bottom_gap = visible.end.saturating_sub(realized.end);
    top_gap.saturating_add(bottom_gap)
}

/// Decision returned by [`decide_scroll_anim_action`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ScrollAnimAction {
    /// Extend the cached frame's realized range to cover the live
    /// viewport via [`crate::window::Window::rebuild_frame_display_dirty`].
    /// Sub-ms on warm caches. Carries the uncovered row count for the
    /// trace event.
    StripRealize {
        /// Total display rows the strip realize must produce specs
        /// for, summed across the top and bottom gaps.
        uncovered_rows: u32,
    },
    /// Strip is too large for a synchronous realize this tick. Reuse
    /// the cached frame as-is; the renderer paints the uncovered rows
    /// as a placeholder strip.
    Placeholder {
        /// Total display rows that will paint as placeholder.
        uncovered_rows: u32,
    },
    /// Cached frame's realized range already covers the viewport — no
    /// new rows needed.
    CoveringReuse,
}

/// Pick the scroll-tick action given the cached frame's realized
/// range, the live viewport row range, and the inline-realize row
/// budget.
#[must_use]
pub(crate) fn decide_scroll_anim_action(
    realized: std::ops::Range<u32>,
    visible: std::ops::Range<u32>,
    max_rows: u32,
) -> ScrollAnimAction {
    let uncovered_rows = uncovered_row_count(realized, visible);
    if uncovered_rows == 0 {
        ScrollAnimAction::CoveringReuse
    } else if uncovered_rows <= max_rows {
        ScrollAnimAction::StripRealize { uncovered_rows }
    } else {
        ScrollAnimAction::Placeholder { uncovered_rows }
    }
}

/// Inputs the dispatcher hands to [`Window::run_scroll_anim_arm`].
/// Mirrors the subset of [`super::worker_outcome_dispatch::WorkerOutcomeDispatchInputs`]
/// the strip-realize call needs — the prev frame, the live viewport,
/// the projection state, and the trace label for the worker-miss
/// reason.
pub(crate) struct ScrollAnimArmInputs<'a> {
    pub prev_frame: FrameDisplay,
    pub viewport_rows: std::ops::Range<u32>,
    pub rope_for_projection: &'a Rope,
    pub revision_for_projection: u64,
    pub decorations: Option<&'a Decorations>,
    pub caret_bytes_for_projection: &'a [usize],
    pub folds_for_projection: &'a [FoldRange],
    pub image_reservations: &'a [ImageRowReservation],
    pub wrap_width_dip: u32,
    pub projection_char_width: f32,
    pub reason_label: &'static str,
}

impl Window {
    /// Section-10 dispatcher arm — given a motion-compatible cached
    /// frame and a viewport row range, choose between
    /// covering-reuse, strip-realize, and placeholder, then emit the
    /// matching trace events.
    pub(crate) fn run_scroll_anim_arm(
        &self,
        inputs: ScrollAnimArmInputs<'_>,
        trace: &PaintTrace,
    ) -> WorkerOutcomeDispatchOutputs {
        let ScrollAnimArmInputs {
            prev_frame,
            viewport_rows,
            rope_for_projection,
            revision_for_projection,
            decorations,
            caret_bytes_for_projection,
            folds_for_projection,
            image_reservations,
            wrap_width_dip,
            projection_char_width,
            reason_label,
        } = inputs;
        let action = decide_scroll_anim_action(
            prev_frame.realized_row_range(),
            viewport_rows.clone(),
            SCROLL_ANIM_STRIP_REALIZE_MAX_ROWS,
        );
        let mut scroll_strip_rows = 0u32;
        let (frame_display, source_label) = match action {
            ScrollAnimAction::CoveringReuse => (prev_frame.clone(), "scroll_anim_reuse"),
            ScrollAnimAction::StripRealize { uncovered_rows } => {
                scroll_strip_rows = uncovered_rows;
                let realize_start = Instant::now();
                let suppressed_table_blocks = self.compute_suppressed_table_blocks();
                let extended = self.rebuild_frame_display_dirty(
                    &prev_frame,
                    &[],
                    rope_for_projection,
                    revision_for_projection,
                    decorations,
                    caret_bytes_for_projection,
                    folds_for_projection,
                    image_reservations,
                    &suppressed_table_blocks,
                    wrap_width_dip,
                    projection_char_width,
                    viewport_rows.clone(),
                    crate::window_paint::VIEWPORT_OVERSCAN_ROWS,
                );
                if crate::paint_trace::is_trace_enabled() {
                    let realized = extended.realized_row_range();
                    let elapsed_us =
                        u64::try_from(realize_start.elapsed().as_micros()).unwrap_or(u64::MAX);
                    crate::paint_trace::log_event(
                        "paint:frame_display:scroll_anim_strip_realize",
                        &format!(
                            "reason={reason_label} uncovered_rows={uncovered_rows} \
                             realized={}..{} elapsed_us={elapsed_us}",
                            realized.start, realized.end,
                        ),
                    );
                }
                trace.mark("frame_display:scroll_anim_strip_realize");
                (extended, "scroll_anim_strip_realize")
            }
            ScrollAnimAction::Placeholder { uncovered_rows } => {
                if crate::paint_trace::is_trace_enabled() {
                    let realized = prev_frame.realized_row_range();
                    crate::paint_trace::log_event(
                        "paint:frame_display:scroll_anim_placeholder",
                        &format!(
                            "reason={reason_label} uncovered_rows={uncovered_rows} \
                             realized={}..{}",
                            realized.start, realized.end,
                        ),
                    );
                }
                trace.mark("frame_display:scroll_anim_placeholder");
                (prev_frame.clone(), "scroll_anim_placeholder")
            }
        };
        let should_skip_cache_seed = matches!(action, ScrollAnimAction::Placeholder { .. });
        if matches!(action, ScrollAnimAction::CoveringReuse) {
            if crate::paint_trace::is_trace_enabled() {
                let realized = prev_frame.realized_row_range();
                crate::paint_trace::log_event(
                    "paint:frame_display:scroll_anim_reuse",
                    &format!(
                        "reason={reason_label} realized={}..{}",
                        realized.start, realized.end,
                    ),
                );
            }
            trace.mark("frame_display:scroll_anim_reuse");
        }
        trace.mark_since_start(
            "frame_ready",
            &format!("source={source_label} reason={reason_label}"),
        );
        WorkerOutcomeDispatchOutputs {
            frame_display,
            frame_source: source_label,
            worker_miss_reason: Some(reason_label),
            should_skip_cache_seed,
            scroll_strip_rows,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncovered_row_count_counts_top_and_bottom_gaps() {
        // Realized 100..200, visible 80..220 → 20 above + 20 below.
        assert_eq!(uncovered_row_count(100..200, 80..220), 40);
        // Visible inside realized → 0.
        assert_eq!(uncovered_row_count(100..200, 110..190), 0);
        // Visible entirely below realized → bottom-extension distance
        // (`visible.end - realized.end`), not just the visible row
        // count. The placeholder decision uses this distance to
        // refuse a huge strip-realize on a far-flick.
        assert_eq!(uncovered_row_count(100..200, 300..360), 160);
        // Empty visible → 0.
        assert_eq!(uncovered_row_count(0..50, 10..10), 0);
    }

    #[test]
    fn strip_realize_fires_for_small_gap_within_budget() {
        // Continuous-scroll case: realized 100..200, viewport 105..210
        // → 10 rows uncovered, well under 80-row budget.
        let action =
            decide_scroll_anim_action(100..200, 105..210, SCROLL_ANIM_STRIP_REALIZE_MAX_ROWS);
        assert_eq!(
            action,
            ScrollAnimAction::StripRealize { uncovered_rows: 10 }
        );
    }

    #[test]
    fn placeholder_fires_when_gap_exceeds_budget() {
        // Fast flick out of cached range — visible 1000..1060, realized
        // 100..200 → 920 uncovered, way over 80-row budget.
        let action =
            decide_scroll_anim_action(100..200, 1000..1060, SCROLL_ANIM_STRIP_REALIZE_MAX_ROWS);
        match action {
            ScrollAnimAction::Placeholder { uncovered_rows } => {
                assert!(uncovered_rows > SCROLL_ANIM_STRIP_REALIZE_MAX_ROWS);
            }
            other => panic!("expected Placeholder, got {other:?}"),
        }
    }

    #[test]
    fn covering_reuse_when_visible_inside_realized() {
        // Cache already covers viewport — no rebuild work needed.
        let action =
            decide_scroll_anim_action(0..400, 150..200, SCROLL_ANIM_STRIP_REALIZE_MAX_ROWS);
        assert_eq!(action, ScrollAnimAction::CoveringReuse);
    }

    #[test]
    fn strip_realize_exact_budget_boundary_still_realizes() {
        // Uncovered rows == budget → still strip realize, not placeholder.
        let action = decide_scroll_anim_action(
            100..200,
            100..(200 + SCROLL_ANIM_STRIP_REALIZE_MAX_ROWS),
            SCROLL_ANIM_STRIP_REALIZE_MAX_ROWS,
        );
        assert_eq!(
            action,
            ScrollAnimAction::StripRealize {
                uncovered_rows: SCROLL_ANIM_STRIP_REALIZE_MAX_ROWS,
            },
        );
    }
}
