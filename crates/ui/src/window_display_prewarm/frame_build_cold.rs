//! Cold-build `FrameDisplay` path with split walker + materialize trace.
//!
//! Sibling of [`super::frame_build`]. Invoked when the cross-pane
//! row-index cache misses and the cross-revision splice fast-path
//! could not bridge the gap. Computes the whole-document
//! [`continuity_display_map::DisplayRowIndex`] via the cheap row-count
//! walker, then materializes only the visible viewport's
//! `DisplayLineSpec`s through the same shared row-index path the
//! cache-hit branch uses.
//!
//! Runs on the [`crate::Window`]-owning UI thread.

use continuity_decorate::Decorations;
use continuity_display_map::{FoldRange, ImageRowReservation, WalkerCallReason};
use continuity_render::{DirectWriteWidthMeasure, FrameDisplay};

use crate::window::Window;

impl Window {
    /// Cold build with the row-count walker phase and the viewport
    /// materialization phase timed separately. Used only on a
    /// row-index cache miss; warmed-cache paths reuse the cached
    /// `Arc<DisplayRowIndex>` and skip the walker entirely.
    ///
    /// Trace emits `row_count_walker`, `row_count_walker_stats`, and
    /// `viewport_materialize` while paint tracing is on.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn cold_build_with_split_trace(
        &self,
        rope: &ropey::Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        wrap_width_dip: u32,
        fallback_char_width_dip: f32,
        visible_rows: std::ops::Range<u32>,
        overscan: u32,
        walker_reason: WalkerCallReason,
    ) -> FrameDisplay {
        let walker_detail = format!("reason={}", walker_reason.as_trace_reason());
        let (row_index, stats, dwrite_stats) = if let Some(format) = self.text_format.as_ref() {
            let mut measure = DirectWriteWidthMeasure::new_with_run_cache(
                self.dwrite.raw(),
                format,
                self.scaled_font_size(),
                continuity_render::DEFAULT_HEADING_SCALE,
                fallback_char_width_dip,
                Some(std::sync::Arc::clone(&self.walker_run_cache)),
                self.font_state,
                crate::window::FONT_LOCALE,
            );
            let scope = crate::paint_trace::is_trace_enabled().then(|| {
                crate::paint_trace::EventScope::with_detail(
                    "row_count_walker",
                    walker_detail.clone(),
                )
            });
            let (ri, s) = FrameDisplay::compute_row_index_measured_with_caches(
                rope,
                revision,
                decorations,
                caret_bytes,
                folds,
                image_reservations,
                self.markdown_render_toggles(),
                wrap_width_dip,
                &mut measure,
                self.font_state.0,
                crate::window::FONT_LOCALE,
                &self.walker_wrap_cache,
                &self.walker_segment_cache,
                walker_reason,
            );
            drop(scope);
            (ri, s, Some(measure.cache_stats()))
        } else {
            let mut measure =
                continuity_display_map::wrap::FixedCharWidth::new(fallback_char_width_dip.max(1.0));
            let scope = crate::paint_trace::is_trace_enabled().then(|| {
                crate::paint_trace::EventScope::with_detail(
                    "row_count_walker",
                    walker_detail.clone(),
                )
            });
            let (ri, s) = FrameDisplay::compute_row_index_measured_with_caches(
                rope,
                revision,
                decorations,
                caret_bytes,
                folds,
                image_reservations,
                self.markdown_render_toggles(),
                wrap_width_dip,
                &mut measure,
                self.font_state.0,
                crate::window::FONT_LOCALE,
                &self.walker_wrap_cache,
                &self.walker_segment_cache,
                walker_reason,
            );
            drop(scope);
            (ri, s, None)
        };
        super::frame_build_stats_emit::emit_walker_stats(&stats, dwrite_stats);
        if let Some(format) = self.text_format.as_ref() {
            let mut measure = DirectWriteWidthMeasure::new_with_run_cache(
                self.dwrite.raw(),
                format,
                self.scaled_font_size(),
                continuity_render::DEFAULT_HEADING_SCALE,
                fallback_char_width_dip,
                Some(std::sync::Arc::clone(&self.walker_run_cache)),
                self.font_state,
                crate::window::FONT_LOCALE,
            );
            let _scope = crate::paint_trace::is_trace_enabled()
                .then(|| crate::paint_trace::EventScope::new("viewport_materialize"));
            FrameDisplay::build_viewport_with_row_index_measured(
                rope,
                revision,
                decorations,
                caret_bytes,
                folds,
                image_reservations,
                self.markdown_render_toggles(),
                wrap_width_dip,
                &mut measure,
                visible_rows,
                overscan,
                row_index,
            )
        } else {
            let mut measure =
                continuity_display_map::wrap::FixedCharWidth::new(fallback_char_width_dip.max(1.0));
            let _scope = crate::paint_trace::is_trace_enabled()
                .then(|| crate::paint_trace::EventScope::new("viewport_materialize"));
            FrameDisplay::build_viewport_with_row_index_measured(
                rope,
                revision,
                decorations,
                caret_bytes,
                folds,
                image_reservations,
                self.markdown_render_toggles(),
                wrap_width_dip,
                &mut measure,
                visible_rows,
                overscan,
                row_index,
            )
        }
    }
}
