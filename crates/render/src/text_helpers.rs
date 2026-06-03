//! Per-line text helpers used by [`crate::Renderer::draw_buffer`].
//!
//! Extracted into a sibling module to keep `renderer.rs` under the 600-line
//! cap. **Thread ownership**: caller is the UI thread (the only owner of
//! the D2D context and DirectWrite resources).

use continuity_display_map::{DisplayLineSpec, SourceByte, SpanRole, SpanStyle};
use continuity_layout::{FontStateId, LayoutCache, LineLayoutKey};
use continuity_text::{Selection, SelectionKind};
use ropey::Rope;
use windows::Win32::Foundation::{BOOL, HMODULE};
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{ID2D1DeviceContext, ID2D1SolidColorBrush};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout, DWRITE_FONT_STYLE_ITALIC,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_HIT_TEST_METRICS, DWRITE_TEXT_RANGE,
};

use crate::display_projection::FrameDisplay;
use crate::Error;

/// Build a layout-cache key for an arbitrary [`continuity_display_map::DisplayLineSpec`],
/// including soft-wrap continuations. The key incorporates the display
/// content stamp plus the spec's `source_byte_start` so two soft-wrap
/// sub-specs of the same source line — each with the same display text
/// in principle — never collide in the cache.
///
/// `wrap_width_dip` is the cache-key wrap width: pass `0` for the
/// soft-wrap *paint* path (where each spec already represents one
/// visual row, so DirectWrite never reflows), or the live
/// `ViewState::wrap_width_key()` for the source-line path (one layout
/// per source line, DirectWrite reflows internally).
pub(crate) fn build_key_for_spec(
    spec: &continuity_display_map::DisplayLineSpec,
    document: u128,
    font_state: FontStateId,
    wrap_width_dip: u32,
) -> LineLayoutKey {
    let mut stamp = spec.content_stamp();
    // Differentiate continuations of the same source line by mixing in
    // the spec's source-byte range — otherwise two visual rows with
    // identical text (e.g. "    " indent) would clobber each other.
    stamp ^= u64::from(spec.source_byte_start.raw()) << 32;
    stamp ^= u64::from(spec.source_byte_end.raw());
    LineLayoutKey {
        document,
        line: spec.source_line.raw(),
        content_stamp: stamp,
        font_state,
        wrap_width_dip,
    }
}

/// Insert a freshly-built `IDWriteTextLayout` built from a `DisplayLineSpec`'s
/// display text, baking `style_runs()` (including heading scale) at build
/// time. This is the *only* layout-build path post-Phase-17.6 cleanup —
/// the legacy `ensure_line_layout` (which fell back to the source-line
/// text + `apply_line_decorations` after-the-fact) is gone.
#[allow(clippy::too_many_arguments)]
pub(crate) fn ensure_line_layout_for_spec(
    cache: &mut LayoutCache,
    factory: &IDWriteFactory,
    spec: &continuity_display_map::DisplayLineSpec,
    key: LineLayoutKey,
    format: &IDWriteTextFormat,
    max_layout_width: f32,
    base_font_size_dip: f32,
    heading_scale: [f32; 6],
) -> Result<(), Error> {
    if cache.get(&key).is_some() {
        return Ok(());
    }
    let text = spec.display_text().to_string();
    let layout = create_layout_with_empty_fallback(factory, format, &text, max_layout_width)?;
    apply_style_runs(
        &layout,
        spec.style_runs(),
        base_font_size_dip,
        heading_scale,
    );
    cache.insert(key, text.into_boxed_str(), layout);
    Ok(())
}

use crate::text_metrics::create_layout_with_empty_fallback;

/// Bake per-segment styles into a freshly-built layout. Called once per
/// (display-content-stamp, font-state) — the cached `IDWriteTextLayout`
/// keeps the styling for as long as it lives in the cache.
fn apply_style_runs(
    layout: &IDWriteTextLayout,
    runs: impl Iterator<
        Item = (
            std::ops::Range<continuity_display_map::DisplayUtf16>,
            SpanStyle,
        ),
    >,
    base_font_size_dip: f32,
    heading_scale: [f32; 6],
) {
    for (range, style) in runs {
        let start = range.start.raw();
        let length = range.end.raw().saturating_sub(start);
        if length == 0 {
            continue;
        }
        let dwrite_range = DWRITE_TEXT_RANGE {
            startPosition: start,
            length,
        };
        let weight = if style.bold {
            DWRITE_FONT_WEIGHT_BOLD
        } else {
            DWRITE_FONT_WEIGHT_NORMAL
        };
        let italic = if style.italic {
            DWRITE_FONT_STYLE_ITALIC
        } else {
            DWRITE_FONT_STYLE_NORMAL
        };
        unsafe {
            let _ = layout.SetFontWeight(weight, dwrite_range);
            let _ = layout.SetFontStyle(italic, dwrite_range);
            if style.strikethrough {
                let _ = layout.SetStrikethrough(true, dwrite_range);
            }
            if style.underline {
                let _ = layout.SetUnderline(true, dwrite_range);
            }
            if let Some(scale) = style_font_scale(style, heading_scale) {
                let size = base_font_size_dip * scale;
                let _ = layout.SetFontSize(size, dwrite_range);
            }
        }
    }
}

fn style_font_scale(style: SpanStyle, heading_scale: [f32; 6]) -> Option<f32> {
    if style.font_scale.as_f32() == 1.0 {
        return None;
    }
    if let SpanRole::Heading(level) = style.role {
        let lvl = level.clamp(1, 6) as usize - 1;
        return Some(heading_scale[lvl]);
    }
    Some(style.font_scale.as_f32())
}

/// Resolve the UTF-16 caret position for `source_byte_in_line` on
/// `line_idx`'s cached layout via the display projection — caret-after-
/// `**` in source maps to caret-at-`h` in display when the markers are
/// hidden. Falls back to identity UTF-8→UTF-16 if the projection has no
/// preimage for the byte (e.g. inside a Hidden segment with no walkable
/// successor).
pub(crate) fn caret_utf16_for_line(
    entry_text: &str,
    frame_display: &FrameDisplay,
    line_idx: usize,
    source_byte_in_line: usize,
) -> usize {
    if let Some(u) =
        frame_display.source_byte_in_line_to_display_utf16(line_idx, source_byte_in_line)
    {
        return u as usize;
    }
    utf8_byte_to_utf16_index(entry_text, source_byte_in_line)
}

/// Resolve the UTF-16 caret position for an absolute source byte inside a
/// concrete [`DisplayLineSpec`]. Used by the soft-wrap path, where one source
/// line can have several display specs.
pub(crate) fn caret_utf16_for_spec(
    entry_text: &str,
    spec: &DisplayLineSpec,
    source_byte: usize,
) -> usize {
    let source = SourceByte::from_usize(source_byte);
    let display_byte = spec.source_to_display(source).or_else(|| {
        let mut probe = source_byte;
        let end = spec.source_byte_end.raw() as usize;
        while probe <= end {
            if let Some(display) = spec.source_to_display(SourceByte::from_usize(probe)) {
                return Some(display);
            }
            probe += 1;
        }
        None
    });
    let Some(display_byte) = display_byte else {
        return utf8_byte_to_utf16_index(
            entry_text,
            source_byte.saturating_sub(spec.source_byte_start.raw() as usize),
        );
    };
    spec.display_byte_to_utf16(display_byte)
        .map(|u| u.raw() as usize)
        .unwrap_or_else(|| {
            let local_display_byte = display_byte.raw() as usize;
            utf8_byte_to_utf16_index(entry_text, local_display_byte)
        })
}

/// Hit-test the X position of a UTF-16 character index within `layout`.
pub(crate) fn hit_test_x(layout: &IDWriteTextLayout, utf16_index: usize) -> Option<f32> {
    let mut x = 0.0f32;
    let mut y = 0.0f32;
    let mut metrics = DWRITE_HIT_TEST_METRICS::default();
    unsafe {
        layout
            .HitTestTextPosition(
                u32::try_from(utf16_index).unwrap_or(0),
                false,
                &mut x,
                &mut y,
                &mut metrics,
            )
            .ok()?;
    }
    Some(x)
}

/// Paint selection rectangles for the given line in layout-local coords.
///
/// Selection byte offsets are translated through the line's source-to-
/// display byte map so the rectangle sits on the correct *display*
/// glyphs (a selection over `**hi**` painted onto a layout built from
/// `"hi"` lands on `hi` rather than at six character widths past the
/// start). The caller is expected to have installed a per-line
/// `SetTransform` so the layout's `(0, 0)` is the line's top-left in
/// body space — no `layout_origin_x` shift is applied here.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_selection_line_with_layout(
    ctx: &ID2D1DeviceContext,
    rope: &Rope,
    text: &str,
    layout: &IDWriteTextLayout,
    line_idx: usize,
    line_height: f32,
    selections: &[Selection],
    brush: &ID2D1SolidColorBrush,
    frame_display: &FrameDisplay,
) {
    for selection in selections {
        let Some((start, end)) = selection_line_span(rope, *selection, line_idx) else {
            continue;
        };
        if start == end {
            continue;
        }
        let start_utf16 = frame_display
            .source_byte_in_line_to_display_utf16(line_idx, start)
            .map(|u| u as usize)
            .unwrap_or_else(|| utf8_byte_to_utf16_index(text, start));
        let end_utf16 = frame_display
            .source_byte_in_line_to_display_utf16(line_idx, end)
            .map(|u| u as usize)
            .unwrap_or_else(|| utf8_byte_to_utf16_index(text, end));
        let left = hit_test_x(layout, start_utf16).unwrap_or(0.0);
        let right = hit_test_x(layout, end_utf16).unwrap_or(left);
        let rect = D2D_RECT_F {
            left: left.min(right),
            top: 0.0,
            right: right.max(left).max(left + 1.0),
            bottom: line_height,
        };
        unsafe {
            ctx.FillRectangle(&rect, brush);
        }
    }
}

fn selection_line_span(
    rope: &Rope,
    selection: Selection,
    line_idx: usize,
) -> Option<(usize, usize)> {
    if selection.is_collapsed() {
        return None;
    }
    if selection.kind == SelectionKind::BlockWise {
        let start_line = selection.anchor.line.min(selection.head.line) as usize;
        let end_line = selection.anchor.line.max(selection.head.line) as usize;
        if line_idx < start_line || line_idx > end_line {
            return None;
        }
        let start = selection
            .anchor
            .byte_in_line
            .min(selection.head.byte_in_line) as usize;
        let end = selection
            .anchor
            .byte_in_line
            .max(selection.head.byte_in_line) as usize;
        return Some((start, end));
    }

    let range = selection.ordered_range();
    let line_start = rope.line_to_byte(line_idx);
    let line_end = line_content_end(rope, line_idx);
    let start = range.start.to_byte_offset(rope).ok()?;
    let end = range.end.to_byte_offset(rope).ok()?;
    let clipped_start = start.max(line_start).min(line_end);
    let clipped_end = end.max(line_start).min(line_end);
    if clipped_start >= clipped_end {
        return None;
    }
    Some((clipped_start - line_start, clipped_end - line_start))
}

fn line_content_end(rope: &Rope, line: usize) -> usize {
    let start = rope.line_to_byte(line);
    let next = if line + 1 < rope.len_lines() {
        rope.line_to_byte(line + 1)
    } else {
        rope.len_bytes()
    };
    let mut end = next;
    let slice = rope.byte_slice(start..next).to_string();
    if slice.ends_with('\n') {
        end = end.saturating_sub(1);
        if slice.ends_with("\r\n") {
            end = end.saturating_sub(1);
        }
    }
    end
}

/// Convert a UTF-8 byte offset within `s` to a UTF-16 code-unit index.
pub(crate) fn utf8_byte_to_utf16_index(s: &str, byte_in_line: usize) -> usize {
    if byte_in_line >= s.len() {
        return s.encode_utf16().count();
    }
    let mut consumed_bytes = 0;
    let mut utf16_index = 0;
    for ch in s.chars() {
        let n = ch.len_utf8();
        if consumed_bytes + n > byte_in_line {
            break;
        }
        consumed_bytes += n;
        utf16_index += ch.len_utf16();
    }
    utf16_index
}

/// Convert a UTF-16 code-unit index within `s` back to a UTF-8 byte
/// offset. Used by [`hit_test_x_to_byte`] to translate DirectWrite's
/// UTF-16 result back into the editor's UTF-8 column space.
#[must_use]
pub fn utf16_index_to_utf8_byte(s: &str, utf16_index: usize) -> usize {
    let mut consumed_utf16 = 0;
    let mut byte = 0;
    for ch in s.chars() {
        let units = ch.len_utf16();
        if consumed_utf16 + units > utf16_index {
            break;
        }
        consumed_utf16 += units;
        byte += ch.len_utf8();
    }
    byte
}

/// Map a horizontal position (in layout-local DIPs) inside the rendered
/// line `text` to a UTF-8 byte offset within that line.
///
/// Builds a one-shot [`IDWriteTextLayout`] for the line, calls
/// `HitTestPoint`, and converts the returned UTF-16 index back to UTF-8.
/// `max_width` is the wrap width in DIPs (or `f32::INFINITY` for no wrap)
/// — pass the same value the renderer used for this line so the metrics
/// match. Returns `None` on a DirectWrite failure or empty input.
#[must_use]
pub fn hit_test_x_to_byte(
    factory: &IDWriteFactory,
    format: &IDWriteTextFormat,
    text: &str,
    x: f32,
    max_width: f32,
) -> Option<usize> {
    let wide: Vec<u16> = text.encode_utf16().collect();
    if wide.is_empty() {
        return Some(0);
    }
    let layout: IDWriteTextLayout = unsafe {
        factory
            .CreateTextLayout(&wide, format, max_width.max(1.0), f32::INFINITY)
            .ok()?
    };
    hit_test_layout_to_byte(&layout, text, x)
}

/// Like [`hit_test_x_to_byte`], but applies `SetFontSize(font_size_dip)` to
/// the layout before hit-testing so the inversion exactly mirrors a painter
/// that draws at an overridden size (e.g. the overlay fields, which paint at
/// the *unzoomed* base size regardless of the format's built size).
///
/// Pass `font_size_dip <= 0.0` to skip the override (identical to
/// [`hit_test_x_to_byte`]). This is the exact inverse of `caret_offset_in_field`
/// in the overlay painter, which builds a plain layout and calls
/// `SetFontSize(font_size_dip)` the same way. Returns `None` on a DirectWrite
/// failure; empty input maps to byte `0`.
#[must_use]
pub fn hit_test_x_to_byte_sized(
    factory: &IDWriteFactory,
    format: &IDWriteTextFormat,
    text: &str,
    x: f32,
    max_width: f32,
    font_size_dip: f32,
) -> Option<usize> {
    let wide: Vec<u16> = text.encode_utf16().collect();
    if wide.is_empty() {
        return Some(0);
    }
    let layout: IDWriteTextLayout = unsafe {
        factory
            .CreateTextLayout(&wide, format, max_width.max(1.0), f32::INFINITY)
            .ok()?
    };
    if font_size_dip > 0.0 {
        let range = DWRITE_TEXT_RANGE {
            startPosition: 0,
            length: wide.len() as u32,
        };
        unsafe {
            let _ = layout.SetFontSize(font_size_dip, range);
        }
    }
    hit_test_layout_to_byte(&layout, text, x)
}

/// Map a horizontal position in a rendered [`DisplayLineSpec`] to a display
/// UTF-8 byte using the same style runs the painter baked into its layout.
#[must_use]
pub fn hit_test_x_to_byte_for_spec(
    factory: &IDWriteFactory,
    format: &IDWriteTextFormat,
    spec: &DisplayLineSpec,
    x: f32,
    max_width: f32,
    base_font_size_dip: f32,
    heading_scale: [f32; 6],
) -> Option<usize> {
    let text = spec.display_text();
    let wide: Vec<u16> = text.encode_utf16().collect();
    if wide.is_empty() {
        return Some(0);
    }
    let layout: IDWriteTextLayout = unsafe {
        factory
            .CreateTextLayout(&wide, format, max_width.max(1.0), f32::INFINITY)
            .ok()?
    };
    apply_style_runs(
        &layout,
        spec.style_runs(),
        base_font_size_dip,
        heading_scale,
    );
    hit_test_layout_to_byte(&layout, text, x)
}

fn hit_test_layout_to_byte(layout: &IDWriteTextLayout, text: &str, x: f32) -> Option<usize> {
    let mut is_trailing = BOOL(0);
    let mut is_inside = BOOL(0);
    let _ = &is_inside;
    let mut metrics = DWRITE_HIT_TEST_METRICS::default();
    unsafe {
        layout
            .HitTestPoint(
                x.max(0.0),
                0.0,
                &mut is_trailing,
                &mut is_inside,
                &mut metrics,
            )
            .ok()?;
    }
    let mut utf16 = metrics.textPosition as usize;
    if is_trailing.as_bool() {
        utf16 = utf16.saturating_add(metrics.length as usize);
    }
    Some(utf16_index_to_utf8_byte(text, utf16))
}

/// D3D11 device factory — tries hardware first, falls back to WARP.
pub(crate) fn create_d3d11_device() -> Result<(ID3D11Device, ID3D11DeviceContext), Error> {
    create_d3d11_device_with(&[D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP])
}

/// WARP-only D3D11 device. Used by the §D pixel canary so byte hashes
/// don't depend on the host GPU. The CPU rasterizer is slower but
/// deterministic across machines (modulo font version + ClearType,
/// which the canary mitigates by forcing grayscale antialiasing and
/// pinning the font family).
pub(crate) fn create_d3d11_device_warp_only() -> Result<(ID3D11Device, ID3D11DeviceContext), Error>
{
    create_d3d11_device_with(&[D3D_DRIVER_TYPE_WARP])
}

fn create_d3d11_device_with(
    drivers: &[windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE],
) -> Result<(ID3D11Device, ID3D11DeviceContext), Error> {
    for driver in drivers {
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        let hr = unsafe {
            D3D11CreateDevice(
                None,
                *driver,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        };
        if hr.is_ok() {
            return Ok((device.expect("device"), context.expect("context")));
        }
    }
    Err(Error::Graphics(windows::core::Error::from_win32()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_to_utf16_basic_ascii() {
        assert_eq!(utf8_byte_to_utf16_index("hello", 0), 0);
        assert_eq!(utf8_byte_to_utf16_index("hello", 3), 3);
        assert_eq!(utf8_byte_to_utf16_index("hello", 5), 5);
    }

    #[test]
    fn utf8_to_utf16_with_multibyte() {
        // "é" is 2 bytes utf-8, 1 code unit utf-16.
        assert_eq!(utf8_byte_to_utf16_index("café", 0), 0);
        assert_eq!(utf8_byte_to_utf16_index("café", 3), 3);
        assert_eq!(utf8_byte_to_utf16_index("café", 5), 4);
    }

    #[test]
    fn revealed_heading_style_keeps_body_font_scale() {
        let style = SpanStyle::heading_revealed(1);
        assert_eq!(style_font_scale(style, crate::DEFAULT_HEADING_SCALE), None);
    }
}
