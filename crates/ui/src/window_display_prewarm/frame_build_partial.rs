//! P18.5b — viewport-priority partial cold build for the
//! [`crate::window_projection_plan::ProjectionBuildKind::ColdPartial`]
//! arm.
//!
//! Sibling of [`super::frame_build_cold`]. Where the cold-build helper
//! walks the entire document's row counts synchronously, this helper
//! walks only the viewport's source-line range and lets the post-paint
//! background fill catch up off-thread. Returns a [`FrameDisplay`]
//! whose underlying row index is partial — `frame_display.row_index()
//! .is_partial()` is `true` until the worker's full frame replaces it
//! on the next paint epilogue.
//!
//! Emits `event:partial_row_index_walk viewport_source_lines=N
//! partial_us=N estimated_total_rows=N` so the trace consumer can
//! confirm the partial path fired.
//!
//! Runs on the UI thread.

use std::ops::Range;

use continuity_decorate::Decorations;
use continuity_display_map::{FoldRange, ImageRowReservation, WalkerCallReason};
use continuity_render::{DirectWriteWidthMeasure, FrameDisplay};
use continuity_text::RopeEditDelta;

use crate::window::Window;

impl Window {
    /// P18.5b — partial cold build: walks only the viewport's
    /// source-line range and returns a [`FrameDisplay`] whose row
    /// index reports `is_partial() == true`. The post-paint dispatch
    /// submits a regular `Cold` worker plan with the `paint_partial_fill`
    /// reason to fill in the rest off-thread; the next paint reads
    /// the worker's full frame.
    ///
    /// The seed-into-row-index-cache step that [`super::frame_build::Window
    /// ::build_frame_display_viewport_cached`] performs is deliberately
    /// skipped here — caching a partial index would let later paints
    /// reuse incomplete row counts when the user navigates around. The
    /// background fill seeds the cache on completion.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_frame_display_viewport_partial_with_trace(
        &self,
        rope: &ropey::Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        wrap_width_dip: u32,
        fallback_char_width_dip: f32,
        visible_rows: Range<u32>,
        overscan: u32,
        viewport_source_range: Range<u32>,
        safety_margin: u32,
    ) -> FrameDisplay {
        let walker_reason = WalkerCallReason::ViewportRealize;
        let walker_detail = format!("reason={}", walker_reason.as_trace_reason());
        let viewport_source_lines = viewport_source_range
            .end
            .saturating_sub(viewport_source_range.start);
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
                    "partial_row_index_walk",
                    walker_detail.clone(),
                )
            });
            let (ri, s) = FrameDisplay::compute_partial_row_index_for_viewport_measured_with_caches(
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
                viewport_source_range,
                safety_margin,
            );
            drop(scope);
            (ri, s, Some(measure.cache_stats()))
        } else {
            let mut measure =
                continuity_display_map::wrap::FixedCharWidth::new(fallback_char_width_dip.max(1.0));
            let scope = crate::paint_trace::is_trace_enabled().then(|| {
                crate::paint_trace::EventScope::with_detail(
                    "partial_row_index_walk",
                    walker_detail.clone(),
                )
            });
            let (ri, s) = FrameDisplay::compute_partial_row_index_for_viewport_measured_with_caches(
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
                viewport_source_range,
                safety_margin,
            );
            drop(scope);
            (ri, s, None)
        };
        super::frame_build_stats_emit::emit_walker_stats(&stats, dwrite_stats);

        // Emit the spec-mandated `event:partial_row_index_walk` line.
        // `viewport_source_lines` is the *requested* range size (before
        // safety-margin padding); `partial_us` and `estimated_total_rows`
        // come straight from the index's `PartialRowIndexState`.
        if crate::paint_trace::is_trace_enabled() {
            let partial_state = row_index.partial_state();
            let estimated_total_rows = partial_state.map(|s| s.scrollbar_estimate).unwrap_or(0);
            let walked_range = partial_state
                .map(|s| s.walked_source_range.clone())
                .unwrap_or(0..0);
            // `partial_us` accumulates from the inline walker via
            // `stats.segment_build_us + stats.measure_us + stats.soft_wrap_walk_us`.
            // The substrate also stores the standalone partial walk time
            // on `PartialWalkOutcome.partial_walk_us`, but that's only
            // accessible at the builder boundary; for the trace event we
            // recompose from the WalkerStats fields the same way the
            // cold-build trace does.
            let partial_us = stats
                .segment_build_us
                .saturating_add(stats.measure_us)
                .saturating_add(stats.soft_wrap_walk_us);
            crate::paint_trace::log_event(
                "event:partial_row_index_walk",
                &format!(
                    "viewport_source_lines={viewport_source_lines} \
                     walked_source_lines={} partial_us={partial_us} \
                     estimated_total_rows={estimated_total_rows}",
                    walked_range.end.saturating_sub(walked_range.start),
                ),
            );
        }

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
                .then(|| crate::paint_trace::EventScope::new("partial_viewport_materialize"));
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
                .then(|| crate::paint_trace::EventScope::new("partial_viewport_materialize"));
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

    /// P18.6 — partial dirty build: paints a viewport-priority row
    /// index now and lets the post-paint worker fill the full index.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_frame_display_viewport_partial_dirty_with_trace(
        &self,
        prev: &FrameDisplay,
        rope: &ropey::Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        wrap_width_dip: u32,
        fallback_char_width_dip: f32,
        visible_rows: Range<u32>,
        overscan: u32,
        viewport_source_range: Range<u32>,
        safety_margin: u32,
        dirty_source_ranges: &[Range<u32>],
    ) -> FrameDisplay {
        let walker_reason = WalkerCallReason::PaintDirty;
        let walker_detail = format!("reason={}", walker_reason.as_trace_reason());
        let viewport_source_lines = viewport_source_range
            .end
            .saturating_sub(viewport_source_range.start);
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
                    "partial_dirty_walk",
                    walker_detail.clone(),
                )
            });
            let (ri, s) =
                FrameDisplay::compute_partial_dirty_row_index_for_viewport_measured_with_caches(
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
                    viewport_source_range,
                    safety_margin,
                    dirty_source_ranges,
                    prev.row_index(),
                );
            drop(scope);
            (ri, s, Some(measure.cache_stats()))
        } else {
            let mut measure =
                continuity_display_map::wrap::FixedCharWidth::new(fallback_char_width_dip.max(1.0));
            let scope = crate::paint_trace::is_trace_enabled().then(|| {
                crate::paint_trace::EventScope::with_detail(
                    "partial_dirty_walk",
                    walker_detail.clone(),
                )
            });
            let (ri, s) =
                FrameDisplay::compute_partial_dirty_row_index_for_viewport_measured_with_caches(
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
                    viewport_source_range,
                    safety_margin,
                    dirty_source_ranges,
                    prev.row_index(),
                );
            drop(scope);
            (ri, s, None)
        };
        super::frame_build_stats_emit::emit_walker_stats(&stats, dwrite_stats);
        emit_partial_variant_trace(
            "event:partial_dirty_walk",
            viewport_source_lines,
            &row_index,
            &stats,
            "dirty_range_count",
            dirty_source_ranges.len(),
        );
        self.build_viewport_from_partial_row_index(
            rope,
            revision,
            decorations,
            caret_bytes,
            folds,
            image_reservations,
            wrap_width_dip,
            fallback_char_width_dip,
            visible_rows,
            overscan,
            row_index,
            "partial_dirty_viewport_materialize",
        )
    }

    /// P18.6 — partial splice build: paints a viewport-priority row
    /// index even when the previous index is partial. The full worker
    /// fill remains a normal Splice when `prev` is full, otherwise Cold.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_frame_display_viewport_partial_splice_with_trace(
        &self,
        prev: &FrameDisplay,
        rope: &ropey::Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        wrap_width_dip: u32,
        fallback_char_width_dip: f32,
        visible_rows: Range<u32>,
        overscan: u32,
        viewport_source_range: Range<u32>,
        safety_margin: u32,
        deltas: &[RopeEditDelta],
    ) -> FrameDisplay {
        let walker_reason = WalkerCallReason::ViewportRealize;
        let walker_detail = format!("reason={}", walker_reason.as_trace_reason());
        let viewport_source_lines = viewport_source_range
            .end
            .saturating_sub(viewport_source_range.start);
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
                    "partial_splice_walk",
                    walker_detail.clone(),
                )
            });
            let (ri, s) =
                FrameDisplay::compute_partial_splice_row_index_for_viewport_measured_with_caches(
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
                    viewport_source_range,
                    safety_margin,
                    deltas,
                    prev.row_index(),
                );
            drop(scope);
            (ri, s, Some(measure.cache_stats()))
        } else {
            let mut measure =
                continuity_display_map::wrap::FixedCharWidth::new(fallback_char_width_dip.max(1.0));
            let scope = crate::paint_trace::is_trace_enabled().then(|| {
                crate::paint_trace::EventScope::with_detail(
                    "partial_splice_walk",
                    walker_detail.clone(),
                )
            });
            let (ri, s) =
                FrameDisplay::compute_partial_splice_row_index_for_viewport_measured_with_caches(
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
                    viewport_source_range,
                    safety_margin,
                    deltas,
                    prev.row_index(),
                );
            drop(scope);
            (ri, s, None)
        };
        super::frame_build_stats_emit::emit_walker_stats(&stats, dwrite_stats);
        emit_partial_variant_trace(
            "event:partial_splice_walk",
            viewport_source_lines,
            &row_index,
            &stats,
            "delta_count",
            deltas.len(),
        );
        self.build_viewport_from_partial_row_index(
            rope,
            revision,
            decorations,
            caret_bytes,
            folds,
            image_reservations,
            wrap_width_dip,
            fallback_char_width_dip,
            visible_rows,
            overscan,
            row_index,
            "partial_splice_viewport_materialize",
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build_viewport_from_partial_row_index(
        &self,
        rope: &ropey::Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        wrap_width_dip: u32,
        fallback_char_width_dip: f32,
        visible_rows: Range<u32>,
        overscan: u32,
        row_index: std::sync::Arc<continuity_display_map::DisplayRowIndex>,
        scope_name: &'static str,
    ) -> FrameDisplay {
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
                .then(|| crate::paint_trace::EventScope::new(scope_name));
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
                .then(|| crate::paint_trace::EventScope::new(scope_name));
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

fn emit_partial_variant_trace(
    event: &'static str,
    viewport_source_lines: u32,
    row_index: &continuity_display_map::DisplayRowIndex,
    stats: &continuity_display_map::WalkerStats,
    extra_key: &'static str,
    extra_value: usize,
) {
    if !crate::paint_trace::is_trace_enabled() {
        return;
    }
    let partial_state = row_index.partial_state();
    let estimated_total_rows = partial_state.map(|s| s.scrollbar_estimate).unwrap_or(0);
    let walked_range = partial_state
        .map(|s| s.walked_source_range.clone())
        .unwrap_or(0..0);
    let partial_us = stats
        .segment_build_us
        .saturating_add(stats.measure_us)
        .saturating_add(stats.soft_wrap_walk_us);
    crate::paint_trace::log_event(
        event,
        &format!(
            "viewport_source_lines={viewport_source_lines} \
             walked_source_lines={} partial_us={partial_us} {extra_key}={extra_value} \
             estimated_total_rows={estimated_total_rows}",
            walked_range.end.saturating_sub(walked_range.start),
        ),
    );
}
