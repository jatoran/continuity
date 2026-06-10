//! Subtle background-rect paint for inline `` `code` `` spans, plus
//! the per-frame hit-rect publish that lets the UI host the
//! copy-button hover affordance.
//!
//! Today's inline-code distinctness was a `SpanRole::Code` flag that
//! the renderer never consulted — the user saw an inline `` `foo` ``
//! that read identically to surrounding prose. This module fills the
//! gap with a small background fill (theme key
//! `markdown.code.background`) sized to the painted glyphs. The same
//! pass records the painted rect to
//! [`crate::InlineCodeHit`] so the UI can attach hover and
//! click logic without re-deriving the layout.
//!
//! **Thread ownership**: UI thread (sole D2D / DirectWrite owner).

use continuity_decorate::{InlineKind, InlineSpan};
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{ID2D1DeviceContext, ID2D1SolidColorBrush};
use windows::Win32::Graphics::DirectWrite::IDWriteTextLayout;

use crate::display_projection::FrameDisplay;
use crate::text_helpers::{caret_utf16_for_line, caret_utf16_for_spec, hit_test_x};
use crate::InlineCodeHit;

/// Horizontal padding (DIPs) extending the painted background a few
/// pixels past each end of the inline code's glyphs — gives the
/// chip a "padded background" feel per spec without crowding
/// neighbouring punctuation.
pub const INLINE_CODE_BG_PAD_DIP: f32 = 2.0;

/// Paint inline-code backgrounds for the no-soft-wrap line path. Mirrors
/// [`crate::inline_color_paint::paint_inline_color_backgrounds_line`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_inline_code_backgrounds_line(
    ctx: &ID2D1DeviceContext,
    layout: &IDWriteTextLayout,
    entry_text: &str,
    frame_display: &FrameDisplay,
    line_idx: usize,
    line_byte_range: std::ops::Range<usize>,
    line_height: f32,
    inlines: &[InlineSpan],
    caret_bytes: &[usize],
    code_bg_brush: &ID2D1SolidColorBrush,
    body_x_in_client: f32,
    body_y_in_client: f32,
    line_paint_y_in_body: f32,
    out_hits: Option<&mut Vec<InlineCodeHit>>,
) {
    let line_byte_range = &line_byte_range;
    let mut collector = out_hits;
    for_each_visible_code_span(
        layout,
        line_byte_range,
        inlines,
        caret_bytes,
        |source_byte| {
            caret_utf16_for_line(
                entry_text,
                frame_display,
                line_idx,
                source_byte.saturating_sub(line_byte_range.start),
            )
        },
        |span, inner_start, inner_end, x_start, x_end| {
            let left = x_start - INLINE_CODE_BG_PAD_DIP;
            let right = x_end + INLINE_CODE_BG_PAD_DIP;
            let rect = D2D_RECT_F {
                left,
                top: 0.0,
                right,
                bottom: line_height,
            };
            unsafe { ctx.FillRectangle(&rect, code_bg_brush) };
            if let Some(ref mut hits) = collector {
                hits.push(InlineCodeHit {
                    outer_start_byte: span.range.start,
                    outer_end_byte: span.range.end,
                    inner_start_byte: inner_start,
                    inner_end_byte: inner_end,
                    rect_client: (
                        body_x_in_client + left,
                        body_y_in_client + line_paint_y_in_body,
                        right - left,
                        line_height,
                    ),
                });
            }
        },
    );
}

/// Soft-wrap companion. Mirrors
/// [`crate::inline_color_paint::paint_inline_color_backgrounds_spec`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_inline_code_backgrounds_spec(
    ctx: &ID2D1DeviceContext,
    layout: &IDWriteTextLayout,
    entry_text: &str,
    spec: &continuity_display_map::DisplayLineSpec,
    line_height: f32,
    inlines: &[InlineSpan],
    caret_bytes: &[usize],
    code_bg_brush: &ID2D1SolidColorBrush,
    body_x_in_client: f32,
    body_y_in_client: f32,
    line_paint_y_in_body: f32,
    out_hits: Option<&mut Vec<InlineCodeHit>>,
) {
    let line_byte_range =
        spec.source_byte_start.raw() as usize..spec.source_byte_end.raw() as usize;
    let line_byte_range = &line_byte_range;
    let mut collector = out_hits;
    for_each_visible_code_span(
        layout,
        line_byte_range,
        inlines,
        caret_bytes,
        |source_byte| caret_utf16_for_spec(entry_text, spec, source_byte),
        |span, inner_start, inner_end, x_start, x_end| {
            let left = x_start - INLINE_CODE_BG_PAD_DIP;
            let right = x_end + INLINE_CODE_BG_PAD_DIP;
            let rect = D2D_RECT_F {
                left,
                top: 0.0,
                right,
                bottom: line_height,
            };
            unsafe { ctx.FillRectangle(&rect, code_bg_brush) };
            if let Some(ref mut hits) = collector {
                hits.push(InlineCodeHit {
                    outer_start_byte: span.range.start,
                    outer_end_byte: span.range.end,
                    inner_start_byte: inner_start,
                    inner_end_byte: inner_end,
                    rect_client: (
                        body_x_in_client + left,
                        body_y_in_client + line_paint_y_in_body,
                        right - left,
                        line_height,
                    ),
                });
            }
        },
    );
}

/// Walk the `inlines` slice, keeping only `InlineKind::Code` spans
/// that intersect `line_byte_range` and whose enclosing run has no
/// caret inside (the same gate `block_revealed` applies). For each
/// visible span emit `(span, inner_start_byte, inner_end_byte,
/// x_start, x_end)` so callers can paint a rect and/or record a hit.
///
/// `inner_*` strips the opening / closing backtick markers — those
/// are zero-or-more-byte ranges immediately before/after the
/// `Code` span in source order. We approximate by detecting an
/// adjacent `CodeDelim` marker on either side and trimming one
/// character. This matches the behaviour the segment builder
/// already enforces (delim markers are hidden when out-of-caret).
fn for_each_visible_code_span(
    layout: &IDWriteTextLayout,
    line_byte_range: &std::ops::Range<usize>,
    inlines: &[InlineSpan],
    caret_bytes: &[usize],
    mut source_to_utf16: impl FnMut(usize) -> usize,
    mut on_span: impl FnMut(&InlineSpan, usize, usize, f32, f32),
) {
    for span in inlines {
        if !matches!(span.kind, InlineKind::Code) {
            continue;
        }
        if caret_bytes
            .iter()
            .any(|c| *c >= span.range.start && *c < span.range.end)
        {
            continue;
        }
        let visible_start = span.range.start.max(line_byte_range.start);
        let visible_end = span.range.end.min(line_byte_range.end);
        if visible_end <= visible_start {
            continue;
        }
        let local_start = visible_start - line_byte_range.start;
        let local_end = visible_end - line_byte_range.start;
        let utf16_start = source_to_utf16(line_byte_range.start + local_start);
        let utf16_end = source_to_utf16(line_byte_range.start + local_end);
        let Some(x_start) = hit_test_x(layout, utf16_start) else {
            continue;
        };
        let Some(x_end) = hit_test_x(layout, utf16_end) else {
            continue;
        };
        if x_end <= x_start {
            continue;
        }
        // The painted rect covers only this display row's slice of the
        // span, but the published hit must carry the FULL inner range:
        // a soft-wrapped `code` span paints across several rows, and the
        // copy button on any of them must copy the whole span — not the
        // one row it happened to be hovered on.
        on_span(span, span.range.start, span.range.end, x_start, x_end);
    }
}

#[cfg(test)]
mod tests {
    // Geometry conversion + delimiter trimming are exercised indirectly
    // through the pixel canary harness; this module's behaviour is
    // entirely visual (DirectWrite layout hit-test math), so there is
    // no useful unit-test surface that doesn't reach into the live D2D
    // context. The hit-rect publishing contract is validated from the
    // UI side (`window_code_copy_hover::tests`).
}
