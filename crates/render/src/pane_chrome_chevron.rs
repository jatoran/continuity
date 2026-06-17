//! Item 8 — overflow scroll chevrons for an overflowing tab strip.
//!
//! Sibling of [`crate::pane_chrome`] (split out to keep that file under
//! the 600-line conventions cap). Paints the `‹` / `›` cells pinned to the
//! strip edges when the tab row is horizontally scrollable.
//!
//! Thread ownership: caller is the UI thread (sole owner of the
//! `ID2D1DeviceContext`).

use windows::Win32::Graphics::Direct2D::Common::{D2D_POINT_2F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_ALIASED,
    D2D1_DRAW_TEXT_OPTIONS_CLIP,
};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory, IDWriteTextFormat, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_WORD_WRAPPING_NO_WRAP,
};

use crate::pane_chrome_layout::{TabStripMetrics, TAB_CHEVRON_WIDTH_DIP};
use crate::Error;

/// `‹` SINGLE LEFT-POINTING ANGLE QUOTATION MARK.
const CHEVRON_LEFT_GLYPH: char = '\u{2039}';
/// `›` SINGLE RIGHT-POINTING ANGLE QUOTATION MARK.
const CHEVRON_RIGHT_GLYPH: char = '\u{203A}';

pub(crate) struct StripChevronGeometry {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) strip_h: f32,
}

/// Paint the left/right scroll chevrons for an overflowing strip. Each
/// chevron is a [`TAB_CHEVRON_WIDTH_DIP`]-wide cell pinned to a strip edge.
/// A cell is dimmed when there is no scroll room in that direction.
pub(crate) fn paint_strip_chevrons(
    d2d: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    metrics: &TabStripMetrics,
    geometry: StripChevronGeometry,
    fg_brush: &ID2D1SolidColorBrush,
) -> Result<(), Error> {
    let can_left = metrics.scroll_offset_dip > 0.5;
    let can_right = metrics.scroll_offset_dip < metrics.max_scroll_offset_dip - 0.5;
    paint_one_chevron(
        d2d,
        dwrite,
        format,
        CHEVRON_LEFT_GLYPH,
        geometry.x + metrics.left_chevron_left(),
        geometry.y,
        geometry.strip_h,
        fg_brush,
        can_left,
    )?;
    paint_one_chevron(
        d2d,
        dwrite,
        format,
        CHEVRON_RIGHT_GLYPH,
        geometry.x + metrics.right_chevron_left(),
        geometry.y,
        geometry.strip_h,
        fg_brush,
        can_right,
    )
}

#[allow(clippy::too_many_arguments)]
fn paint_one_chevron(
    d2d: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    glyph: char,
    cell_left: f32,
    y: f32,
    strip_h: f32,
    fg_brush: &ID2D1SolidColorBrush,
    enabled: bool,
) -> Result<(), Error> {
    let cell = D2D_RECT_F {
        left: cell_left,
        top: y,
        right: cell_left + TAB_CHEVRON_WIDTH_DIP,
        bottom: y + strip_h,
    };
    let mut text = String::new();
    text.push(glyph);
    let wide: Vec<u16> = text.encode_utf16().collect();
    let layout = unsafe {
        dwrite.CreateTextLayout(
            &wide,
            format,
            TAB_CHEVRON_WIDTH_DIP.max(1.0),
            strip_h.max(1.0),
        )?
    };
    unsafe {
        layout.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
        layout.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
    }
    // Disabled chevrons paint at reduced alpha so the affordance reads as
    // "no further to scroll" without disappearing (the cell stays clickable
    // — the hit-test clamps a no-op scroll).
    unsafe {
        if !enabled {
            fg_brush.SetOpacity(0.4);
        }
        d2d.PushAxisAlignedClip(&cell, D2D1_ANTIALIAS_MODE_ALIASED);
        d2d.DrawTextLayout(
            D2D_POINT_2F {
                x: cell.left,
                y: cell.top,
            },
            &layout,
            fg_brush,
            D2D1_DRAW_TEXT_OPTIONS_CLIP,
        );
        d2d.PopAxisAlignedClip();
        if !enabled {
            fg_brush.SetOpacity(1.0);
        }
    }
    Ok(())
}
