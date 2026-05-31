//! Direct row-index helpers for input paths.
//!
//! These methods run on the [`crate::Window`]-owning UI thread. They
//! reuse the same DirectWrite measurement inputs as paint but only for
//! a selected source-line set, or for viewport materialization from an
//! already-compatible row index.

use std::ops::Range;
use std::sync::Arc;

use continuity_decorate::Decorations;
use continuity_display_map::{DisplayRowIndex, FoldRange, ImageRowReservation};
use continuity_render::{DirectWriteWidthMeasure, FrameDisplay};

use crate::window::Window;

impl Window {
    /// Refresh selected source-line counts on top of `previous`
    /// without invoking the whole-document row-count walker.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn refresh_frame_display_row_index_source_lines(
        &self,
        previous: &DisplayRowIndex,
        source_lines: &[u32],
        rope: &ropey::Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        wrap_width_dip: u32,
        fallback_char_width_dip: f32,
    ) -> Option<Arc<DisplayRowIndex>> {
        if let Some(format) = self.text_format.as_ref() {
            let mut measure = DirectWriteWidthMeasure::new_with_run_cache(
                self.dwrite.raw(),
                format,
                self.scaled_font_size(),
                continuity_render::DEFAULT_HEADING_SCALE,
                fallback_char_width_dip,
                Some(Arc::clone(&self.walker_run_cache)),
                self.font_state,
                crate::window::FONT_LOCALE,
            );
            return FrameDisplay::refresh_row_index_source_lines_measured_with_caches(
                previous,
                source_lines,
                rope,
                revision,
                decorations,
                caret_bytes,
                folds,
                image_reservations,
                wrap_width_dip,
                &mut measure,
                self.font_state.0,
                crate::window::FONT_LOCALE,
                &self.walker_wrap_cache,
                &self.walker_segment_cache,
            );
        }

        let mut measure =
            continuity_display_map::wrap::FixedCharWidth::new(fallback_char_width_dip.max(1.0));
        FrameDisplay::refresh_row_index_source_lines_measured_with_caches(
            previous,
            source_lines,
            rope,
            revision,
            decorations,
            caret_bytes,
            folds,
            image_reservations,
            wrap_width_dip,
            &mut measure,
            self.font_state.0,
            crate::window::FONT_LOCALE,
            &self.walker_wrap_cache,
            &self.walker_segment_cache,
        )
    }

    /// Materialize a viewport from an already-compatible row index.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_frame_display_from_row_index(
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
        row_index: Arc<DisplayRowIndex>,
    ) -> FrameDisplay {
        if let Some(format) = self.text_format.as_ref() {
            let mut measure = DirectWriteWidthMeasure::new(
                self.dwrite.raw(),
                format,
                self.scaled_font_size(),
                continuity_render::DEFAULT_HEADING_SCALE,
                fallback_char_width_dip,
            );
            return FrameDisplay::build_viewport_with_row_index_measured(
                rope,
                revision,
                decorations,
                caret_bytes,
                folds,
                image_reservations,
                wrap_width_dip,
                &mut measure,
                visible_rows,
                overscan,
                row_index,
            );
        }

        let mut measure =
            continuity_display_map::wrap::FixedCharWidth::new(fallback_char_width_dip.max(1.0));
        FrameDisplay::build_viewport_with_row_index_measured(
            rope,
            revision,
            decorations,
            caret_bytes,
            folds,
            image_reservations,
            wrap_width_dip,
            &mut measure,
            visible_rows,
            overscan,
            row_index,
        )
    }
}
