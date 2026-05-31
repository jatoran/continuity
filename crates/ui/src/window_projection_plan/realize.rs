//! Inline realization of a [`super::ProjectionBuildKind`]. Used when
//! the projection worker missed; emits the same per-branch
//! `paint:frame_display:*` trace events the pre-ε.5c inline rebuild
//! tree did.
//!
//! Thread ownership: UI thread of one window. Each arm dispatches to
//! the matching `Window::build_*` / `Window::rebuild_*` helper and
//! marks the inline trace.

use std::ops::Range;

use continuity_decorate::Decorations;
use continuity_display_map::{FoldRange, ImageRowReservation};
use continuity_render::FrameDisplay;
use ropey::Rope;

use super::dirty_spill::spill_if_large;
use super::ProjectionBuildKind;
use crate::paint_trace::PaintTrace;
use crate::window::Window;
use crate::window_paint_selection_reveal::log_selection_reveal_rebuild;
use crate::window_row_splice::log_row_index_splice;

impl Window {
    /// Inline-realize a [`ProjectionBuildKind`]. Used when the worker
    /// missed; emits the same per-branch paint-trace events the
    /// pre-ε.5c inline tree did.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn realize_projection_build_kind(
        &self,
        kind: &ProjectionBuildKind,
        rope: &Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        wrap_width_dip: u32,
        projection_char_width: f32,
        viewport_rows: &Range<u32>,
        prior_carets_for_trace: &[usize],
        selection_reveal_dirty: &[u32],
        trace: &PaintTrace,
    ) -> FrameDisplay {
        match kind {
            ProjectionBuildKind::CacheHit(cached) => {
                trace.mark("frame_display:cache_hit");
                cached.clone()
            }
            ProjectionBuildKind::SelectionRebuild { prev, dirty } => {
                let suppressed_table_blocks = self.compute_suppressed_table_blocks();
                let rebuilt = self.rebuild_frame_display_dirty(
                    prev,
                    dirty,
                    rope,
                    revision,
                    decorations,
                    caret_bytes,
                    folds,
                    image_reservations,
                    &suppressed_table_blocks,
                    wrap_width_dip,
                    projection_char_width,
                    viewport_rows.clone(),
                    crate::window_paint::VIEWPORT_OVERSCAN_ROWS,
                );
                log_selection_reveal_rebuild(
                    selection_reveal_dirty,
                    viewport_rows,
                    prior_carets_for_trace,
                    caret_bytes,
                );
                trace.mark("frame_display:selection_reveal_rebuild");
                rebuilt
            }
            ProjectionBuildKind::Dirty { prev, dirty } => self.realize_dirty(
                prev,
                dirty,
                rope,
                revision,
                decorations,
                caret_bytes,
                folds,
                image_reservations,
                wrap_width_dip,
                projection_char_width,
                viewport_rows,
                trace,
            ),
            ProjectionBuildKind::ViewportRealize { prev, dirty } => {
                let suppressed_table_blocks = self.compute_suppressed_table_blocks();
                let rebuilt = self.rebuild_frame_display_dirty(
                    prev,
                    dirty,
                    rope,
                    revision,
                    decorations,
                    caret_bytes,
                    folds,
                    image_reservations,
                    &suppressed_table_blocks,
                    wrap_width_dip,
                    projection_char_width,
                    viewport_rows.clone(),
                    crate::window_paint::VIEWPORT_OVERSCAN_ROWS,
                );
                if crate::paint_trace::is_trace_enabled() {
                    let prev_realized = prev.realized_row_range();
                    let detail = format!(
                        "viewport={}..{} prev_realized={}..{} dirty_count={} source_lines={}",
                        viewport_rows.start,
                        viewport_rows.end,
                        prev_realized.start,
                        prev_realized.end,
                        dirty.len(),
                        prev.row_index().source_line_count(),
                    );
                    crate::paint_trace::log_event("paint:frame_display:viewport_realize", &detail);
                }
                trace.mark("frame_display:viewport_realize");
                rebuilt
            }
            ProjectionBuildKind::Splice {
                prev,
                splice,
                deltas,
            } => {
                let prev_lines = prev.row_index().source_line_count();
                let suppressed_table_blocks = self.compute_suppressed_table_blocks();
                let rebuilt = self.rebuild_frame_display_spliced(
                    prev,
                    splice,
                    rope,
                    revision,
                    decorations,
                    caret_bytes,
                    folds,
                    image_reservations,
                    &suppressed_table_blocks,
                    wrap_width_dip,
                    projection_char_width,
                    viewport_rows.clone(),
                    crate::window_paint::VIEWPORT_OVERSCAN_ROWS,
                );
                log_row_index_splice(
                    splice,
                    deltas,
                    prev_lines,
                    rope.len_lines() as u32,
                    viewport_rows,
                );
                trace.mark("frame_display:row_index_splice");
                rebuilt
            }
            ProjectionBuildKind::ColdPartial {
                viewport_source_range,
                safety_margin,
            } => {
                let partial = self.build_frame_display_viewport_partial_with_trace(
                    rope,
                    revision,
                    decorations,
                    caret_bytes,
                    folds,
                    image_reservations,
                    wrap_width_dip,
                    projection_char_width,
                    viewport_rows.clone(),
                    crate::window_paint::VIEWPORT_OVERSCAN_ROWS,
                    viewport_source_range.clone(),
                    *safety_margin,
                );
                trace.mark("frame_display:cold_partial_build");
                partial
            }
            ProjectionBuildKind::DirtyPartial {
                prev,
                viewport_source_range,
                safety_margin,
                dirty_source_ranges,
            } => {
                let partial = self.build_frame_display_viewport_partial_dirty_with_trace(
                    prev,
                    rope,
                    revision,
                    decorations,
                    caret_bytes,
                    folds,
                    image_reservations,
                    wrap_width_dip,
                    projection_char_width,
                    viewport_rows.clone(),
                    crate::window_paint::VIEWPORT_OVERSCAN_ROWS,
                    viewport_source_range.clone(),
                    *safety_margin,
                    dirty_source_ranges,
                );
                trace.mark("frame_display:dirty_partial_build");
                partial
            }
            ProjectionBuildKind::SplicePartial {
                prev,
                viewport_source_range,
                safety_margin,
                deltas,
                ..
            } => {
                let partial = self.build_frame_display_viewport_partial_splice_with_trace(
                    prev,
                    rope,
                    revision,
                    decorations,
                    caret_bytes,
                    folds,
                    image_reservations,
                    wrap_width_dip,
                    projection_char_width,
                    viewport_rows.clone(),
                    crate::window_paint::VIEWPORT_OVERSCAN_ROWS,
                    viewport_source_range.clone(),
                    *safety_margin,
                    deltas,
                );
                trace.mark("frame_display:splice_partial_build");
                partial
            }
            ProjectionBuildKind::Cold => {
                let cold = self.build_frame_display_viewport_cached(
                    Some(self.buffer_id),
                    rope,
                    revision,
                    decorations,
                    caret_bytes,
                    folds,
                    image_reservations,
                    wrap_width_dip,
                    projection_char_width,
                    viewport_rows.clone(),
                    crate::window_paint::VIEWPORT_OVERSCAN_ROWS,
                    continuity_display_map::WalkerCallReason::PaintCold,
                );
                if crate::paint_trace::is_trace_enabled() {
                    let realized = cold.realized_row_range();
                    let detail = format!(
                        "requested={}..{} realized={}..{} overscan={}",
                        viewport_rows.start,
                        viewport_rows.end,
                        realized.start,
                        realized.end,
                        crate::window_paint::VIEWPORT_OVERSCAN_ROWS,
                    );
                    crate::paint_trace::log_event("paint:frame_display:viewport_realize", &detail);
                }
                trace.mark("frame_display:cold_build");
                cold
            }
        }
    }

    /// Inline-realize a [`ProjectionBuildKind::Dirty`]. Large dirty
    /// sets spill to the projection worker via
    /// [`spill_if_large`]; everything else rebuilds inline against
    /// `prev`.
    #[allow(clippy::too_many_arguments)]
    fn realize_dirty(
        &self,
        prev: &FrameDisplay,
        dirty: &[u32],
        rope: &Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        wrap_width_dip: u32,
        projection_char_width: f32,
        viewport_rows: &Range<u32>,
        trace: &PaintTrace,
    ) -> FrameDisplay {
        if let Some(spilled) = spill_if_large(
            self,
            prev,
            dirty,
            rope,
            revision,
            decorations,
            caret_bytes,
            folds,
            image_reservations,
            wrap_width_dip,
            projection_char_width,
            viewport_rows,
            trace,
        ) {
            return spilled;
        }
        let suppressed_table_blocks = self.compute_suppressed_table_blocks();
        let rebuilt = self.rebuild_frame_display_dirty(
            prev,
            dirty,
            rope,
            revision,
            decorations,
            caret_bytes,
            folds,
            image_reservations,
            &suppressed_table_blocks,
            wrap_width_dip,
            projection_char_width,
            viewport_rows.clone(),
            crate::window_paint::VIEWPORT_OVERSCAN_ROWS,
        );
        if crate::paint_trace::is_trace_enabled() {
            let lo = dirty.first().copied().unwrap_or(0);
            let hi = dirty.last().copied().unwrap_or(0);
            let detail = format!(
                "dirty_count={} dirty_span={lo}..={hi} viewport={}..{}",
                dirty.len(),
                viewport_rows.start,
                viewport_rows.end,
            );
            crate::paint_trace::log_event("paint:frame_display:dirty_rebuild", &detail);
        }
        trace.mark("frame_display:dirty_rebuild");
        rebuilt
    }
}
