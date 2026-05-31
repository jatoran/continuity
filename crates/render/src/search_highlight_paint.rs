//! Body search-result highlight painter.
//!
//! Search ranges arrive as source-byte spans from the UI thread. The
//! renderer clips each range to the concrete display line being painted and
//! projects through the display map before drawing, so hidden markdown
//! markers, replacements, folds, and soft-wrap continuations stay coherent.
//!
//! Thread ownership: caller is the UI thread.

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{ID2D1DeviceContext, ID2D1SolidColorBrush};
use windows::Win32::Graphics::DirectWrite::IDWriteTextLayout;

use crate::display_projection::FrameDisplay;
use crate::text_helpers::{caret_utf16_for_line, caret_utf16_for_spec, hit_test_x};

/// One find-bar match range to paint in the editor body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchHighlightRangeDraw {
    /// Inclusive source-byte start in the buffer.
    pub start_byte: usize,
    /// Exclusive source-byte end in the buffer.
    pub end_byte: usize,
    /// `true` for the find bar's current match.
    pub is_active: bool,
}

/// D2D inputs shared by one search-highlight paint call.
pub(crate) struct SearchHighlightPaint<'a> {
    ctx: &'a ID2D1DeviceContext,
    layout: &'a IDWriteTextLayout,
    line_height: f32,
    ranges: &'a [SearchHighlightRangeDraw],
    match_brush: &'a ID2D1SolidColorBrush,
    active_brush: &'a ID2D1SolidColorBrush,
}

impl<'a> SearchHighlightPaint<'a> {
    pub(crate) fn new(
        ctx: &'a ID2D1DeviceContext,
        layout: &'a IDWriteTextLayout,
        line_height: f32,
        ranges: &'a [SearchHighlightRangeDraw],
        match_brush: &'a ID2D1SolidColorBrush,
        active_brush: &'a ID2D1SolidColorBrush,
    ) -> Self {
        Self {
            ctx,
            layout,
            line_height,
            ranges,
            match_brush,
            active_brush,
        }
    }
}

/// Paint source-line search highlights for the non-wrap text path.
pub(crate) fn paint_search_highlights_line(
    paint: &SearchHighlightPaint<'_>,
    entry_text: &str,
    frame_display: &FrameDisplay,
    line_idx: usize,
) {
    let Some(spec) = frame_display.line(line_idx) else {
        return;
    };
    let line_start = spec.source_byte_start.raw() as usize;
    paint_ranges(
        paint,
        line_start..spec.source_byte_end.raw() as usize,
        |source_byte| {
            caret_utf16_for_line(
                entry_text,
                frame_display,
                line_idx,
                source_byte.saturating_sub(line_start),
            )
        },
    );
}

/// Paint search highlights for one concrete display spec in the soft-wrap
/// path.
pub(crate) fn paint_search_highlights_spec(
    paint: &SearchHighlightPaint<'_>,
    entry_text: &str,
    spec: &continuity_display_map::DisplayLineSpec,
) {
    paint_ranges(
        paint,
        spec.source_byte_start.raw() as usize..spec.source_byte_end.raw() as usize,
        |source_byte| caret_utf16_for_spec(entry_text, spec, source_byte),
    );
}

fn paint_ranges(
    paint: &SearchHighlightPaint<'_>,
    line_byte_range: std::ops::Range<usize>,
    mut source_to_utf16: impl FnMut(usize) -> usize,
) {
    if paint.ranges.is_empty() {
        return;
    }
    let line_start = line_byte_range.start;
    let line_end = line_byte_range.end;
    let first_candidate = paint
        .ranges
        .partition_point(|range| range.end_byte <= line_start);
    for active_pass in [false, true] {
        for range in &paint.ranges[first_candidate..] {
            if range.start_byte >= line_end {
                break;
            }
            if range.is_active != active_pass {
                continue;
            }
            let start = range.start_byte.max(line_start);
            let end = range.end_byte.min(line_end);
            if end <= start {
                continue;
            }
            let utf16_start = source_to_utf16(start);
            let utf16_end = source_to_utf16(end);
            let Some(x_start) = hit_test_x(paint.layout, utf16_start) else {
                continue;
            };
            let Some(x_end) = hit_test_x(paint.layout, utf16_end) else {
                continue;
            };
            if x_end <= x_start {
                continue;
            }
            let rect = D2D_RECT_F {
                left: x_start,
                top: 0.0,
                right: x_end.max(x_start + 1.0),
                bottom: paint.line_height,
            };
            let brush = if range.is_active {
                paint.active_brush
            } else {
                paint.match_brush
            };
            unsafe { paint.ctx.FillRectangle(&rect, brush) };
        }
    }
}
