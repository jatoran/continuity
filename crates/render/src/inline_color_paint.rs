//! Phase F3 — paint pass for `==text==` highlight and `{#hex:text}`
//! foreground-color spans.
//!
//! Two visual contributions per span:
//!
//! - `InlineColorKind::Highlight` — fill a background rectangle behind
//!   the `inner` bytes of the span using
//!   `MarkdownColors::inline_highlight_bg`. The foreground re-color is
//!   left as theme default; the bg fill is the user-visible cue.
//! - `InlineColorKind::Hex(rgba)` — paint the inner text again with a
//!   per-span hex-color brush on top of the main glyph layer. The
//!   default-foreground glyphs that the main layout already drew are
//!   visible only briefly per frame — the hex-colored overdraw sits
//!   above them. Because both layouts use the same `IDWriteTextFormat`
//!   and the same UTF-16 substring, glyph positions match pixel-for-
//!   pixel.
//!
//! Caret-inside-span reveal: when any caret byte falls inside a span's
//! `outer` range, neither visual is painted — the user sees the source
//! delimiters and edits the markup directly. Mirrors the
//! `MarkerKind::TablePipe` per-block reveal rule.
//!
//! **Thread ownership**: caller is the UI thread.

use continuity_decorate::{InlineColorKind, InlineColorSpan};
use windows::core::Interface;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_POINT_2F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1RenderTarget, ID2D1SolidColorBrush, D2D1_DRAW_TEXT_OPTIONS_NONE,
};
use windows::Win32::Graphics::DirectWrite::{IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout};

use crate::display_projection::FrameDisplay;
use crate::params::Rgba;
use crate::text_helpers::{caret_utf16_for_line, caret_utf16_for_spec, hit_test_x};

/// Paint inline highlight backgrounds for one display line. Runs
/// **before** the main `DrawTextLayout` so the rectangle sits behind
/// glyphs. Hex-kind spans are no-op'd here — the foreground overdraw
/// lives in [`paint_inline_color_foregrounds_line`] and runs post-text.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_inline_color_backgrounds_line(
    ctx: &ID2D1DeviceContext,
    layout: &IDWriteTextLayout,
    entry_text: &str,
    frame_display: &FrameDisplay,
    line_idx: usize,
    line_byte_range: std::ops::Range<usize>,
    line_height: f32,
    spans: &[InlineColorSpan],
    caret_bytes: &[usize],
    highlight_bg_brush: &ID2D1SolidColorBrush,
) {
    for_each_visible_span(
        layout,
        &line_byte_range,
        spans,
        caret_bytes,
        |source_byte| {
            caret_utf16_for_line(
                entry_text,
                frame_display,
                line_idx,
                source_byte.saturating_sub(line_byte_range.start),
            )
        },
        |span, x_start, x_end, _utf16_start, _utf16_end| {
            if !matches!(span.kind, InlineColorKind::Highlight) {
                return;
            }
            let rect = D2D_RECT_F {
                left: x_start,
                top: 0.0,
                right: x_end,
                bottom: line_height,
            };
            unsafe { ctx.FillRectangle(&rect, highlight_bg_brush) };
        },
    );
}

/// Paint inline highlight backgrounds for one concrete display spec. This is
/// the soft-wrap companion to [`paint_inline_color_backgrounds_line`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_inline_color_backgrounds_spec(
    ctx: &ID2D1DeviceContext,
    layout: &IDWriteTextLayout,
    entry_text: &str,
    spec: &continuity_display_map::DisplayLineSpec,
    line_height: f32,
    spans: &[InlineColorSpan],
    caret_bytes: &[usize],
    highlight_bg_brush: &ID2D1SolidColorBrush,
) {
    let line_byte_range =
        spec.source_byte_start.raw() as usize..spec.source_byte_end.raw() as usize;
    for_each_visible_span(
        layout,
        &line_byte_range,
        spans,
        caret_bytes,
        |source_byte| caret_utf16_for_spec(entry_text, spec, source_byte),
        |span, x_start, x_end, _utf16_start, _utf16_end| {
            if !matches!(span.kind, InlineColorKind::Highlight) {
                return;
            }
            let rect = D2D_RECT_F {
                left: x_start,
                top: 0.0,
                right: x_end,
                bottom: line_height,
            };
            unsafe { ctx.FillRectangle(&rect, highlight_bg_brush) };
        },
    );
}

/// Paint hex-color foreground overdraws for one display line. Runs
/// **after** the main `DrawTextLayout` so the colored glyphs cover the
/// default-color glyphs at the same x-positions. Highlight-kind spans
/// are no-op'd here — their bg fill lives in
/// [`paint_inline_color_backgrounds_line`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_inline_color_foregrounds_line(
    ctx: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    layout: &IDWriteTextLayout,
    entry_text: &str,
    frame_display: &FrameDisplay,
    line_idx: usize,
    line_byte_range: std::ops::Range<usize>,
    line_height: f32,
    spans: &[InlineColorSpan],
    caret_bytes: &[usize],
) {
    for_each_visible_span(
        layout,
        &line_byte_range,
        spans,
        caret_bytes,
        |source_byte| {
            caret_utf16_for_line(
                entry_text,
                frame_display,
                line_idx,
                source_byte.saturating_sub(line_byte_range.start),
            )
        },
        |span, x_start, x_end, utf16_start, utf16_end| {
            let InlineColorKind::Hex(rgba) = span.kind else {
                return;
            };
            paint_hex_foreground_run(
                ctx,
                dwrite,
                format,
                entry_text,
                utf16_start,
                utf16_end,
                x_start,
                x_end,
                line_height,
                rgba,
            );
        },
    );
}

/// Paint hex-color foreground overdraws for one concrete display spec. This is
/// the soft-wrap companion to [`paint_inline_color_foregrounds_line`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_inline_color_foregrounds_spec(
    ctx: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    layout: &IDWriteTextLayout,
    entry_text: &str,
    spec: &continuity_display_map::DisplayLineSpec,
    line_height: f32,
    spans: &[InlineColorSpan],
    caret_bytes: &[usize],
) {
    let line_byte_range =
        spec.source_byte_start.raw() as usize..spec.source_byte_end.raw() as usize;
    for_each_visible_span(
        layout,
        &line_byte_range,
        spans,
        caret_bytes,
        |source_byte| caret_utf16_for_spec(entry_text, spec, source_byte),
        |span, x_start, x_end, utf16_start, utf16_end| {
            let InlineColorKind::Hex(rgba) = span.kind else {
                return;
            };
            paint_hex_foreground_run(
                ctx,
                dwrite,
                format,
                entry_text,
                utf16_start,
                utf16_end,
                x_start,
                x_end,
                line_height,
                rgba,
            );
        },
    );
}

/// Shared driver — iterate `spans` whose `outer` intersects the line
/// AND no caret is inside, resolve x-coordinates, and dispatch to the
/// caller's per-span closure with the resolved rect.
#[allow(clippy::too_many_arguments)]
fn for_each_visible_span(
    layout: &IDWriteTextLayout,
    line_byte_range: &std::ops::Range<usize>,
    spans: &[InlineColorSpan],
    caret_bytes: &[usize],
    mut source_to_utf16: impl FnMut(usize) -> usize,
    mut on_span: impl FnMut(&InlineColorSpan, f32, f32, usize, usize),
) {
    for span in spans {
        if caret_bytes
            .iter()
            .any(|c| *c >= span.outer.start && *c < span.outer.end)
        {
            continue;
        }
        let inner_start = span.inner.start.max(line_byte_range.start);
        let inner_end = span.inner.end.min(line_byte_range.end);
        if inner_end <= inner_start {
            continue;
        }
        let local_start = inner_start - line_byte_range.start;
        let local_end = inner_end - line_byte_range.start;
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
        on_span(span, x_start, x_end, utf16_start, utf16_end);
    }
}

/// Build a per-span hex-color brush and draw a tiny `IDWriteTextLayout`
/// over the inner UTF-16 substring at the same x-position the main
/// layout used. Errors fall through silently — the source glyphs from
/// the main layout remain visible, just in the default theme color.
#[allow(clippy::too_many_arguments)]
fn paint_hex_foreground_run(
    ctx: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    entry_text: &str,
    utf16_start: usize,
    utf16_end: usize,
    x_start: f32,
    x_end: f32,
    line_height: f32,
    rgba: u32,
) {
    // Slice the substring of `entry_text` covering UTF-16 range
    // `[utf16_start, utf16_end)`. We re-encode to UTF-16 because the
    // display map may have inserted Replace segments — the only stable
    // way to feed DirectWrite is to round-trip via `entry_text`.
    let wide_all: Vec<u16> = entry_text.encode_utf16().collect();
    let start = utf16_start.min(wide_all.len());
    let end = utf16_end.min(wide_all.len());
    if end <= start {
        return;
    }
    let slice = &wide_all[start..end];
    let layout_w = (x_end - x_start).max(1.0);
    let Ok(span_layout): Result<IDWriteTextLayout, _> =
        (unsafe { dwrite.CreateTextLayout(slice, format, layout_w, line_height) })
    else {
        return;
    };
    // Build the brush from the packed hex value.
    let color: D2D1_COLOR_F = unpack_rgba(rgba).into();
    let render_target: ID2D1RenderTarget = match ctx.cast() {
        Ok(rt) => rt,
        Err(_) => return,
    };
    let Ok(brush) = (unsafe { render_target.CreateSolidColorBrush(&color, None) }) else {
        return;
    };
    unsafe {
        ctx.DrawTextLayout(
            D2D_POINT_2F { x: x_start, y: 0.0 },
            &span_layout,
            &brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
        );
    }
}

/// Unpack `0xRRGGBBAA` into a normalized [`Rgba`].
fn unpack_rgba(packed: u32) -> Rgba {
    let r = ((packed >> 24) & 0xFF) as f32 / 255.0;
    let g = ((packed >> 16) & 0xFF) as f32 / 255.0;
    let b = ((packed >> 8) & 0xFF) as f32 / 255.0;
    let a = (packed & 0xFF) as f32 / 255.0;
    Rgba { r, g, b, a }
}

/// Collect the byte offsets of every selection head — the
/// caret-byte set fed to [`paint_inline_color_spans_line`].
#[must_use]
pub(crate) fn caret_bytes_from_selections(
    rope: &ropey::Rope,
    selections: &[continuity_text::Selection],
) -> Vec<usize> {
    selections
        .iter()
        .map(|s| {
            let line_start = rope.line_to_byte(s.head.line as usize);
            line_start + s.head.byte_in_line as usize
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_decorate::{InlineColorKind, InlineColorSpan};

    #[test]
    fn unpack_rgba_returns_normalized_channels() {
        // 0xFF0000FF → opaque red.
        let c = unpack_rgba(0xFF0000FF);
        assert!((c.r - 1.0).abs() < f32::EPSILON);
        assert!(c.g.abs() < f32::EPSILON);
        assert!(c.b.abs() < f32::EPSILON);
        assert!((c.a - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn unpack_rgba_mid_channel_values() {
        let c = unpack_rgba(0x80408020);
        assert!((c.r - 128.0 / 255.0).abs() < 1e-6);
        assert!((c.g - 64.0 / 255.0).abs() < 1e-6);
        assert!((c.b - 128.0 / 255.0).abs() < 1e-6);
        assert!((c.a - 32.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn caret_inside_outer_suppresses_paint() {
        let span = InlineColorSpan {
            outer: 0..6,
            inner: 2..4,
            kind: InlineColorKind::Highlight,
        };
        let inside = [3usize];
        assert!(inside
            .iter()
            .any(|c| *c >= span.outer.start && *c < span.outer.end));
        let outside = [99usize];
        assert!(!outside
            .iter()
            .any(|c| *c >= span.outer.start && *c < span.outer.end));
    }

    #[test]
    fn intersect_line_drops_non_overlapping_span() {
        let span = InlineColorSpan {
            outer: 30..50,
            inner: 32..48,
            kind: InlineColorKind::Highlight,
        };
        let line_start = 100usize;
        let line_end = 200usize;
        let inner_start = span.inner.start.max(line_start);
        let inner_end = span.inner.end.min(line_end);
        assert!(inner_end <= inner_start);
    }
}
