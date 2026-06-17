//! Geometry-shift scroll anchor.
//!
//! `scroll_y_dip` is an absolute display-row coordinate. The document's
//! projected height can change *between paints* without any deliberate
//! scroll or caret move — the projection that serves a paint alternates
//! between cache/worker/inline frames whose whole-document row geometry
//! differs (the caret-byte set that reveals a block as a unit is excluded
//! from frame motion-compatibility, so frames with the block revealed and
//! collapsed are treated as interchangeable). When the rows *above* the
//! caret line change count, the same `scroll_y_dip` maps to a different
//! document position and the viewport jumps — the "viewport jumps while
//! typing in a long file" bug (`perf-snapshots/trace_20260616-202018`: the
//! caret source line resolving to display row 909 one paint and 972 the
//! next, the whole-document total swinging 1130<->1204 between `cache_hit`
//! and `dirty_partial` frames).
//!
//! This applies the project's "layout shifts preserve caret-line screen y"
//! principle (`.docs/design/principles.md`) to that *implicit* per-paint
//! reflow — the explicit reflow triggers (font scale, wrap width, pane
//! resize) already route through [`crate::Window::with_caret_line_anchored`].
//!
//! ## Why post-resolution
//!
//! The geometry that this paint will draw is only known once the frame is
//! resolved (the resolver picks cache vs worker vs inline). Comparing a
//! pre-resolution estimate against the previous paint computes a zero delta
//! and corrects nothing. So [`Window::apply_geometry_anchor`] runs *after*
//! frame resolution + cache seeding: it reads the caret source line's first
//! display row from the resolved frame, shifts `scroll_y_dip` by the row
//! delta versus the previous paint, then **re-realizes** the corrected
//! viewport from the now-cached row index (cheap — no cold walk) so the
//! painted specs match the corrected scroll with no gaps. The re-realize is
//! guarded on a cached row index; when it is not cheaply available the
//! anchor degrades to a no-op (never worse than the un-anchored paint).
//!
//! The compensation is keyed on the source line's *first* display row
//! (rows above the caret line), never the caret's own row — so typing that
//! grows the caret line's own wrap depth, and pure caret motion within the
//! line, do not trigger a shift. It is immune to deliberate scrolls (wheel,
//! scrollbar, caret reveal, doc-end snap): those move `scroll_y_dip` but not
//! the geometry above the caret, so the row delta is zero and the
//! deliberate scroll is preserved. When the caret changes source line the
//! anchor stands down and re-baselines, leaving placement to the caret
//! reveal path.
//!
//! Thread ownership: UI-thread-only. Mutates `self.view.scroll_y_dip`,
//! `frame_display`, `last_painted_frame_display`, and
//! `prev_paint_caret_line_anchor`.

use std::ops::Range;

use continuity_decorate::Decorations;
use continuity_display_map::{FoldRange, ImageRowReservation};
use continuity_render::FrameDisplay;
use ropey::Rope;

use super::caret_visibility::approximate_caret_continuation_row;
use crate::display_prewarm_cache::PrewarmQuery;
use crate::window_paint::{visible_display_row_range, VIEWPORT_OVERSCAN_ROWS};
use crate::Window;

/// UI-thread state for the per-paint geometry-shift anchor and reveal
/// handoff.
#[derive(Debug, Default)]
pub(crate) struct GeometryAnchorState {
    /// Whether the primary caret was on-screen at the previous painted
    /// frame's end; lets layout-shift scroll anchoring skip re-targeting
    /// an already-off-screen caret.
    pub(crate) caret_was_on_screen_prior_frame: bool,
    /// `(caret_source_line, source_line_first_display_row)` captured at
    /// the end of the previous focused paint.
    pub(crate) previous_paint_caret_line_anchor: Option<(u32, u32)>,
    /// Set by the post-edit/motion caret reveal to request that the next
    /// focused paint guarantee the primary caret is inside the viewport.
    pub(crate) pending_caret_reveal: bool,
}

/// Pure scroll math: shift `scroll_y_dip` by the caret source line's
/// first-display-row delta, clamped into `[0, max_scroll]`. Factored out
/// so the compensation contract is unit-testable without a `Window`.
#[must_use]
pub(crate) fn geometry_shift_scroll(
    scroll_y_dip: f32,
    prev_first_row: u32,
    now_first_row: u32,
    line_height: f32,
    content_height_dip: f32,
    viewport_height_dip: f32,
) -> f32 {
    let delta = (now_first_row as f32 - prev_first_row as f32) * line_height;
    let max_scroll = (content_height_dip - viewport_height_dip).max(0.0);
    (scroll_y_dip + delta).clamp(0.0, max_scroll)
}

/// Pure scroll math: clamp `scroll_y_dip` so the caret's display row sits
/// inside `[scroll, scroll + viewport]`. Returns the input unchanged when
/// the caret is already visible. This is the **visibility floor** the
/// geometry anchor applies (after holding the caret line) when a reveal was
/// requested — measured against the *resolved* frame's geometry so a wrong
/// pre-paint estimate can never leave the caret stranded off screen.
/// Factored out for unit testing without a `Window`.
#[must_use]
pub(crate) fn clamp_scroll_to_caret_visible(
    scroll_y_dip: f32,
    caret_display_row: u32,
    line_height: f32,
    content_height_dip: f32,
    viewport_height_dip: f32,
) -> f32 {
    let caret_top = caret_display_row as f32 * line_height;
    let caret_bottom = caret_top + line_height;
    let max_scroll = (content_height_dip - viewport_height_dip).max(0.0);
    let adjusted = if caret_top < scroll_y_dip {
        // Caret row is above the viewport top — scroll up to it.
        caret_top
    } else if caret_bottom > scroll_y_dip + viewport_height_dip {
        // Caret row is below the viewport bottom — scroll down to it.
        caret_bottom - viewport_height_dip
    } else {
        scroll_y_dip
    };
    adjusted.clamp(0.0, max_scroll)
}

impl Window {
    /// Hold the caret source line at the same screen y across an implicit
    /// geometry reflow, then re-baseline the anchor for the next paint.
    /// Call in the focused-pane paint *after* `resolve_paint_frame_display`
    /// and `seed_paint_caches_after_resolve` (so the row index is cached)
    /// and *before* the draw. May replace `*frame_display` with a frame
    /// re-realized for the corrected scroll.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn apply_geometry_anchor(
        &mut self,
        frame_display: &mut FrameDisplay,
        display_query: &PrewarmQuery,
        rope: &Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        suppressed_table_blocks: &[Range<usize>],
        wrap_width_dip: u32,
        char_width_dip: f32,
    ) {
        let Some(snap) = self.editor.snapshot(self.buffer_id) else {
            self.geometry_anchor.previous_paint_caret_line_anchor = None;
            self.geometry_anchor.pending_caret_reveal = false;
            return;
        };
        let Some(sel) = snap.selections().first().copied() else {
            self.geometry_anchor.previous_paint_caret_line_anchor = None;
            self.geometry_anchor.pending_caret_reveal = false;
            return;
        };
        let caret_line = sel.head.line;
        let now_first_row = frame_display.first_display_line_index_for_source(caret_line as usize);
        let line_height = self.effective_line_height();
        let total_rows = frame_display.display_line_count();
        let content_height = self
            .estimated_content_height()
            .max(total_rows.max(1) as f32 * line_height);

        let mut target_scroll = self.view.scroll_y_dip;

        // (1) Hold the caret line at the same screen y across an implicit
        // geometry reflow (rows above the caret appearing / disappearing as
        // the served frame's geometry swings while typing).
        if let Some((prev_line, prev_first_row)) =
            self.geometry_anchor.previous_paint_caret_line_anchor
        {
            if prev_line == caret_line && now_first_row != prev_first_row {
                target_scroll = geometry_shift_scroll(
                    target_scroll,
                    prev_first_row,
                    now_first_row,
                    line_height,
                    content_height,
                    self.view.viewport_height_dip,
                );
            }
        }

        // (2) Visibility floor: when this paint follows a caret reveal
        // request, guarantee the caret's display row is on screen *in the
        // resolved frame*. Holding the line (step 1) preserves the caret's
        // screen y when it was already visible, but a bad baseline or the
        // first paint after a click can leave it off screen — and the
        // pre-paint reveal's estimate may have wrongly concluded "visible"
        // against a different geometry. This is the authoritative check.
        let reveal_pending = self.geometry_anchor.pending_caret_reveal;
        self.geometry_anchor.pending_caret_reveal = false;
        if reveal_pending {
            let caret_display_row = frame_display
                .display_line_index_for_source_pos(
                    caret_line as usize,
                    sel.head.byte_in_line as usize,
                )
                .unwrap_or_else(|| {
                    // The caret's row is not realized in the resolved frame —
                    // it sits on a continuation row outside the materialized
                    // viewport (e.g. End on a long wrapped line that extends
                    // below the screen). Approximate the within-line wrap
                    // offset so the clamp reveals the caret's actual row
                    // instead of stopping at the line's first row.
                    now_first_row.saturating_add(approximate_caret_continuation_row(
                        rope,
                        caret_line as usize,
                        sel.head.byte_in_line as usize,
                        wrap_width_dip,
                        char_width_dip,
                    ))
                });
            target_scroll = clamp_scroll_to_caret_visible(
                target_scroll,
                caret_display_row,
                line_height,
                content_height,
                self.view.viewport_height_dip,
            );
        }

        if (target_scroll - self.view.scroll_y_dip).abs() > 0.5 {
            if crate::paint_trace::is_trace_enabled() {
                crate::paint_trace::log_event(
                    "geometry_anchor_shift",
                    &format!(
                        "caret_line={caret_line} prev_first_row={} \
                         now_first_row={now_first_row} reveal={reveal_pending} \
                         scroll={:.0}->{target_scroll:.0}",
                        self.geometry_anchor
                            .previous_paint_caret_line_anchor
                            .map_or(now_first_row, |(_, r)| r),
                        self.view.scroll_y_dip,
                    ),
                );
            }
            self.view.scroll_y_dip = target_scroll;
            let visible_rows = visible_display_row_range(
                target_scroll,
                self.view.viewport_height_dip,
                line_height,
            );
            // Re-realize the corrected viewport from the *resolved frame's
            // own* whole-document row index (dirty rebuild with no dirty
            // lines): the geometry is unchanged, only which specs are
            // materialized, so this reuses the index and clean specs — no
            // cold walk, no row-index-cache dependency.
            let rebuilt = self.rebuild_frame_display_dirty(
                frame_display,
                &[],
                rope,
                revision,
                decorations,
                caret_bytes,
                folds,
                image_reservations,
                suppressed_table_blocks,
                wrap_width_dip,
                char_width_dip,
                visible_rows,
                VIEWPORT_OVERSCAN_ROWS,
            );
            *frame_display = rebuilt;
            // Keep motion-reuse + the next anchor pass consistent with what
            // was actually drawn.
            self.last_painted_frame_display = Some((display_query.clone(), frame_display.clone()));
        }

        // Re-baseline: the re-realize reuses the same row index, so the
        // caret line's first display row is unchanged by the correction.
        self.geometry_anchor.previous_paint_caret_line_anchor = Some((
            caret_line,
            frame_display.first_display_line_index_for_source(caret_line as usize),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::{clamp_scroll_to_caret_visible, geometry_shift_scroll};

    const LH: f32 = 22.0;
    const VH: f32 = 700.0;
    const CONTENT: f32 = 1204.0 * LH;

    #[test]
    fn unchanged_row_keeps_scroll() {
        assert_eq!(
            geometry_shift_scroll(5000.0, 900, 900, LH, CONTENT, VH),
            5000.0
        );
    }

    #[test]
    fn rows_added_above_caret_scroll_down_to_hold_line() {
        // 63 extra rows materialized above the caret line (the 1130->1204
        // swing seen in the trace): scroll must increase by 63*LH so the
        // caret line stays put rather than jumping up.
        let out = geometry_shift_scroll(5000.0, 909, 972, LH, CONTENT, VH);
        assert!((out - (5000.0 + 63.0 * LH)).abs() < 1e-3);
    }

    #[test]
    fn rows_removed_above_caret_scroll_up_to_hold_line() {
        let out = geometry_shift_scroll(5000.0, 972, 909, LH, CONTENT, VH);
        assert!((out - (5000.0 - 63.0 * LH)).abs() < 1e-3);
    }

    #[test]
    fn clamps_at_top() {
        // A large upward correction can't drive scroll below zero.
        assert_eq!(geometry_shift_scroll(100.0, 50, 0, LH, CONTENT, VH), 0.0);
    }

    #[test]
    fn clamps_at_bottom() {
        let max = (CONTENT - VH).max(0.0);
        let out = geometry_shift_scroll(max, 0, 10_000, LH, CONTENT, VH);
        assert_eq!(out, max);
    }

    #[test]
    fn visible_caret_keeps_scroll() {
        // Caret row 1010 at scroll 1000*LH=22000: caret_top=22220 sits in
        // [22000, 22700) — already visible, scroll unchanged.
        let scroll = 1000.0 * LH;
        assert_eq!(
            clamp_scroll_to_caret_visible(scroll, 1010, LH, CONTENT, VH),
            scroll
        );
    }

    #[test]
    fn caret_above_viewport_scrolls_up_to_it() {
        // The stuck-off-screen regression: anchor held the caret line at a
        // negative screen y. The clamp must pull the scroll up so the caret
        // row sits at the viewport top.
        let scroll = 23049.0; // caret row 1006 -> top 22132 < scroll => above
        let out = clamp_scroll_to_caret_visible(scroll, 1006, LH, CONTENT, VH);
        assert!((out - 1006.0 * LH).abs() < 1e-3, "got {out}");
    }

    #[test]
    fn caret_below_viewport_scrolls_down_to_it() {
        let scroll = 1000.0 * LH; // viewport rows ~1000..1031
        let caret_row = 1080; // bottom = 1081*LH > scroll+VH
        let out = clamp_scroll_to_caret_visible(scroll, caret_row, LH, CONTENT, VH);
        assert!(
            (out - ((caret_row as f32 + 1.0) * LH - VH)).abs() < 1e-3,
            "got {out}"
        );
    }

    #[test]
    fn visibility_clamp_never_goes_negative() {
        assert_eq!(clamp_scroll_to_caret_visible(50.0, 0, LH, CONTENT, VH), 0.0);
    }
}
