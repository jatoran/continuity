//! Text layout and caret measurement helpers for overlay chrome.

use windows::Win32::Graphics::Direct2D::Common::{D2D_POINT_2F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1SolidColorBrush, D2D1_DRAW_TEXT_OPTIONS_CLIP,
};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory, IDWriteTextFormat, DWRITE_HIT_TEST_METRICS, DWRITE_TEXT_RANGE, DWRITE_TRIMMING,
    DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_NO_WRAP,
};

use crate::Error;

/// Draw `text` inside `rect` at a pinned `font_size_dip`, regardless of the
/// size baked into `format`. Overlays are chrome: their layout rects are
/// fixed (e.g. `overlay_render::ROW_HEIGHT`), so the text must stay a fixed
/// size or it overflows the row when the body `format` is zoomed up. Mirrors
/// the status-bar `SetFontSize` technique. `font_size_dip <= 0` leaves the
/// format's own size.
///
/// Overlay rows have a fixed height, so text must never wrap onto a second
/// visual line. When `ellipsize` is set, the clipped tail is replaced with an
/// ellipsis. Editable fields pass `ellipsize = false` so caret math stays
/// consistent with what is painted.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_text_sized(
    ctx: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    text: &str,
    rect: D2D_RECT_F,
    font_size_dip: f32,
    brush: &ID2D1SolidColorBrush,
    ellipsize: bool,
) -> Result<(), Error> {
    if text.is_empty() {
        return Ok(());
    }
    let wide: Vec<u16> = text.encode_utf16().collect();
    let width = (rect.right - rect.left).max(0.0);
    let height = (rect.bottom - rect.top).max(0.0);
    let layout = unsafe { dwrite.CreateTextLayout(&wide, format, width, height)? };
    if font_size_dip > 0.0 {
        let range = DWRITE_TEXT_RANGE {
            startPosition: 0,
            length: wide.len() as u32,
        };
        unsafe {
            let _ = layout.SetFontSize(font_size_dip, range);
        }
    }
    unsafe {
        let _ = layout.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        if ellipsize {
            if let Ok(sign) = dwrite.CreateEllipsisTrimmingSign(format) {
                let trimming = DWRITE_TRIMMING {
                    granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                    delimiter: 0,
                    delimiterCount: 0,
                };
                let _ = layout.SetTrimming(&trimming, &sign);
            }
        }
    }
    unsafe {
        ctx.DrawTextLayout(
            D2D_POINT_2F {
                x: rect.left,
                y: rect.top,
            },
            &layout,
            brush,
            D2D1_DRAW_TEXT_OPTIONS_CLIP,
        );
    }
    Ok(())
}

pub(super) fn caret_offset_in_field(
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    text: &str,
    caret_byte: usize,
    font_size_dip: f32,
) -> Option<f32> {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let layout = unsafe {
        dwrite
            .CreateTextLayout(&wide, format, f32::INFINITY, f32::INFINITY)
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
    let utf16_index = utf8_byte_to_utf16_index(text, caret_byte);
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

pub(super) fn utf8_byte_to_utf16_index(s: &str, byte_in_line: usize) -> usize {
    if byte_in_line >= s.len() {
        return s.encode_utf16().count();
    }
    let mut consumed = 0;
    let mut idx = 0;
    for ch in s.chars() {
        let len = ch.len_utf8();
        if consumed + len > byte_in_line {
            break;
        }
        consumed += len;
        idx += ch.len_utf16();
    }
    idx
}
