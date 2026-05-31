//! Small paint helpers used by `paint_buffer_history_panel_no_present`.
//! Sibling of `buffer_history_panel.rs`. Pulled out so the parent
//! paint module stays under the 600-line cap.

use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_POINT_2F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_ALIASED, D2D1_DRAW_TEXT_OPTIONS_CLIP,
};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteTextFormat, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_WORD_WRAPPING_NO_WRAP,
};

use super::{BufferHistoryPanelDraw, PanelRect};
use crate::params::Rgba;
use crate::renderer::Renderer;
use crate::Error;

/// Produce a short "today · this week · this month · older"
/// breakdown for the ruler's right-aligned hint label.
pub(super) fn ruler_bucket_hint(draw: &BufferHistoryPanelDraw) -> Option<String> {
    if draw.rows.is_empty() {
        return None;
    }
    let now = draw.now_ms;
    let mut today = 0_u32;
    let mut this_week = 0_u32;
    let mut this_month = 0_u32;
    let mut older = 0_u32;
    const MS_PER_DAY: i64 = 24 * 60 * 60 * 1_000;
    for row in &draw.rows {
        let Some(&ts) = row.snapshot_times_ms.last() else {
            continue;
        };
        let age = now.saturating_sub(ts).max(0);
        if age < MS_PER_DAY {
            today += 1;
        } else if age < 7 * MS_PER_DAY {
            this_week += 1;
        } else if age < 30 * MS_PER_DAY {
            this_month += 1;
        } else {
            older += 1;
        }
    }
    let buffers = draw.rows.len();
    let buffer_label = if buffers == 1 { "buffer" } else { "buffers" };
    Some(format!(
        "{buffers} {buffer_label} · {today} today · {this_week} this week · \
         {this_month} this month · {older} older"
    ))
}

pub(super) fn draw_label(
    renderer: &Renderer,
    base_format: &IDWriteTextFormat,
    rect: &PanelRect,
    text: &str,
    brush: &ID2D1SolidColorBrush,
    alignment: windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT,
) -> Result<(), Error> {
    let saved = unsafe { base_format.GetTextAlignment() };
    unsafe {
        let _ = base_format.SetTextAlignment(alignment);
        let body = D2D_RECT_F {
            left: rect.x + 6.0,
            top: rect.y,
            right: rect.x + rect.w - 6.0,
            bottom: rect.y + rect.h,
        };
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let layout = renderer.dwrite_factory.CreateTextLayout(
            &utf16,
            base_format,
            (body.right - body.left).max(1.0),
            (body.bottom - body.top).max(1.0),
        );
        let _ = base_format.SetTextAlignment(saved);
        let layout = layout?;
        layout.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        renderer
            .d2d_context
            .PushAxisAlignedClip(&body, D2D1_ANTIALIAS_MODE_ALIASED);
        renderer.d2d_context.DrawTextLayout(
            D2D_POINT_2F {
                x: body.left,
                y: body.top,
            },
            &layout,
            brush,
            D2D1_DRAW_TEXT_OPTIONS_CLIP,
        );
        renderer.d2d_context.PopAxisAlignedClip();
    }
    Ok(())
}

/// Draw a multi-line label by splitting the source on `\n` so each
/// rope line gets its own 18-DIP row instead of being collapsed to
/// one DWrite line. Truncates when the rect can't fit more rows.
pub(super) fn draw_multiline_label(
    renderer: &Renderer,
    base_format: &IDWriteTextFormat,
    rect: &PanelRect,
    text: &str,
    brush: &ID2D1SolidColorBrush,
) -> Result<(), Error> {
    const ROW_H: f32 = 18.0;
    let max_rows = (rect.h / ROW_H).floor() as usize;
    if max_rows == 0 {
        return Ok(());
    }
    for (i, line) in text.lines().take(max_rows).enumerate() {
        let row = PanelRect {
            x: rect.x,
            y: rect.y + (i as f32) * ROW_H,
            w: rect.w,
            h: ROW_H,
        };
        draw_label(
            renderer,
            base_format,
            &row,
            line,
            brush,
            DWRITE_TEXT_ALIGNMENT_LEADING,
        )?;
    }
    Ok(())
}

pub(super) fn panel_rect_to_d2d(r: PanelRect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: r.x,
        top: r.y,
        right: r.x + r.w,
        bottom: r.y + r.h,
    }
}

pub(super) fn rgba_to_d2d(c: Rgba) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
}
