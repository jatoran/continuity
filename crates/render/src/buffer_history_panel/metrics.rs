//! Zoom-resolved geometry for the buffer-history panel.
//!
//! The panel paints its text with a `text_format` whose glyph size is
//! `base_font * font_size_scale`. To keep every text-bearing cell sized
//! for those glyphs at any zoom, this module multiplies the base DIP
//! constants (declared on the parent module) by the same `scale` once
//! per layout pass and hands the result to the layout math, the
//! scrollbar layout, and the paint pass. Centralizing it here means the
//! pointer hit-test (which runs the same layout function) and the paint
//! stay pixel-consistent, and it keeps the parent paint module under the
//! 600-line cap.

use super::scrollbar;
use super::{
    HEADER_ROW_HEIGHT_DIP, LANE_HEIGHT_DIP, PANEL_PAD_DIP, PREVIEW_BAND_HEIGHT_DIP,
    RULER_HEIGHT_DIP, SNAPSHOT_DOT_RADIUS_DIP, STRIP_HEIGHT_DIP, SUBTITLE_ROW_HEIGHT_DIP,
    TITLE_COLUMN_WIDTH_DIP, TITLE_ROW_HEIGHT_DIP, TITLE_SUBTITLE_GAP_DIP, TITLE_TOP_PAD_DIP,
};

/// Resolved, zoom-scaled geometry for the buffer-history panel.
///
/// Computed once per layout pass from [`super::BufferHistoryPanelDraw::
/// scale`] and shared by the layout math, the scrollbar layout, and the
/// paint pass so paint and the pointer hit-test stay pixel-consistent.
#[derive(Copy, Clone, Debug)]
pub(crate) struct BufferHistoryMetrics {
    /// Zoom factor (`>= 1.0` zooms in, `< 1.0` zooms out), already
    /// clamped to a sane range.
    pub scale: f32,
    /// Scaled ruler-band height.
    pub ruler_height: f32,
    /// Scaled ruler header chip-line height.
    pub header_row_height: f32,
    /// Scaled per-lane row height.
    pub lane_height: f32,
    /// Scaled title-column width.
    pub title_column_width: f32,
    /// Scaled inner horizontal padding.
    pub panel_pad: f32,
    /// Scaled snapshot-dot radius.
    pub dot_radius: f32,
    /// Scaled timeline-strip height.
    pub strip_height: f32,
    /// Scaled title sub-row height.
    pub title_row_height: f32,
    /// Scaled subtitle sub-row height.
    pub subtitle_row_height: f32,
    /// Scaled top padding above the title.
    pub title_top_pad: f32,
    /// Scaled gap between title and subtitle.
    pub title_subtitle_gap: f32,
    /// Scaled bottom preview-band height.
    pub preview_band_height: f32,
    /// Scaled right-edge scrollbar gutter.
    pub scrollbar_gutter: f32,
}

/// Resolve the panel's zoom-scaled geometry from a raw `scale` (clamped
/// to a sane range so a degenerate input never collapses the layout).
#[must_use]
pub(crate) fn resolved_metrics(scale: f32) -> BufferHistoryMetrics {
    let scale = if scale.is_finite() {
        scale.clamp(0.25, 8.0)
    } else {
        1.0
    };
    BufferHistoryMetrics {
        scale,
        ruler_height: RULER_HEIGHT_DIP * scale,
        header_row_height: HEADER_ROW_HEIGHT_DIP * scale,
        lane_height: LANE_HEIGHT_DIP * scale,
        title_column_width: TITLE_COLUMN_WIDTH_DIP * scale,
        panel_pad: PANEL_PAD_DIP * scale,
        dot_radius: SNAPSHOT_DOT_RADIUS_DIP * scale,
        strip_height: STRIP_HEIGHT_DIP * scale,
        title_row_height: TITLE_ROW_HEIGHT_DIP * scale,
        subtitle_row_height: SUBTITLE_ROW_HEIGHT_DIP * scale,
        title_top_pad: TITLE_TOP_PAD_DIP * scale,
        title_subtitle_gap: TITLE_SUBTITLE_GAP_DIP * scale,
        preview_band_height: PREVIEW_BAND_HEIGHT_DIP * scale,
        scrollbar_gutter: scrollbar::SCROLLBAR_GUTTER_DIP * scale,
    }
}
