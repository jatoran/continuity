//! Scaled-text minimap D2D paint dispatch.
//!
//! Reads geometry from [`crate::minimap::MinimapLayout`] and emits one
//! tiny [`IDWriteTextLayout`] per visible source line. Drawn with a
//! dedicated [`IDWriteTextFormat`] at [`MINIMAP_FONT_SIZE_DIP`] so the
//! buffer's per-line layout cache (sized for the editor body) doesn't
//! get polluted with minimap entries.
//!
//! The painter is intentionally re-creating the small text format on
//! every frame: minimap repaint is bounded by visible-line count, not
//! by buffer size, so the cost stays well inside the §15 keypress →
//! pixel budget. A future optimization can cache the format on the
//! `Renderer` if `dhat` flags allocations here.
//!
//! Thread ownership: render thread of the owning window (caller).

use ropey::Rope;
use windows::core::Interface;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_POINT_2F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1RenderTarget, ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_ALIASED,
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_DRAW_TEXT_OPTIONS_NONE,
};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory, IDWriteTextFormat, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_NORMAL,
};

use crate::error::Error;
use crate::minimap::{
    MinimapColors, MinimapLayout, MINIMAP_FONT_SIZE_DIP, MINIMAP_INNER_PADDING_DIP,
    MINIMAP_LINE_HEIGHT_DIP,
};

/// Paint the scaled-text minimap into the focused pane's body rect.
///
/// Caller has already set the device-context transform so `(0, 0)` is
/// the pane body's top-left in client coords; coordinates in `layout`
/// are pane-local DIPs.
///
/// Failures swallow internally: the minimap is decoration and must
/// never block an editor frame. The function returns `Ok(())` even if
/// individual layout creations fail — the result is a partially-drawn
/// strip, never a missing pane paint.
///
/// # Errors
///
/// Returns [`Error::Graphics`] only when a brush or text format fails
/// to allocate (the strip is degenerate so there is no point drawing).
pub(crate) fn paint_minimap_scaled(
    ctx: &ID2D1DeviceContext,
    factory: &IDWriteFactory,
    base_format: &IDWriteTextFormat,
    rope: &Rope,
    layout: &MinimapLayout,
    colors: MinimapColors,
) -> Result<(), Error> {
    let (rx, ry, rw, rh) = layout.rect;
    if rw <= 0.0 || rh <= 0.0 {
        return Ok(());
    }
    let render_target: ID2D1RenderTarget = ctx.cast()?;
    let mk_brush = |rgba: crate::params::Rgba| -> Result<ID2D1SolidColorBrush, Error> {
        let c: D2D1_COLOR_F = rgba.into();
        Ok(unsafe { render_target.CreateSolidColorBrush(&c, None)? })
    };
    let bg_brush = mk_brush(colors.bg)?;
    let fg_brush = mk_brush(colors.fg)?;
    let indicator_brush = mk_brush(colors.viewport_indicator)?;

    let strip = D2D_RECT_F {
        left: rx,
        top: ry,
        right: rx + rw,
        bottom: ry + rh,
    };
    unsafe { ctx.FillRectangle(&strip, &bg_brush) };

    // Clip glyph paint to the strip so a long source line can't bleed
    // into the outline sidebar (or off-screen) when the minimap is the
    // outermost right-edge consumer.
    unsafe {
        ctx.PushAxisAlignedClip(&strip, D2D1_ANTIALIAS_MODE_ALIASED);
    }

    // Build a minimap-local text format. We can't `SetFontSize` on an
    // existing IDWriteTextFormat (the size is locked at creation), so
    // we read the family name off the base format and re-create at the
    // minimap size. Failure ⇒ skip glyph paint, still draw the strip
    // background + indicator (better than nothing on the screen).
    if let Ok(format) = build_minimap_format(factory, base_format) {
        // Antialiased text inside the minimap reads better than the
        // aliased clip mode applied to the strip background — D2D
        // takes the inner mode from BeginDraw's outer setting, so we
        // flip back to per-primitive AA for the glyph pass and revert
        // before popping the clip.
        let prior_mode = unsafe { ctx.GetAntialiasMode() };
        unsafe { ctx.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) };

        let text_left = rx + MINIMAP_INNER_PADDING_DIP;
        let text_width = (rw - 2.0 * MINIMAP_INNER_PADDING_DIP).max(1.0);
        let first = layout.first_visible_line.min(layout.total_lines);
        let last = layout.last_visible_line.min(layout.total_lines);
        let rope_lines = rope.len_lines() as u64;
        let last = last.min(rope_lines);
        for line_idx in first..last {
            let y = ry + (line_idx as f32) * layout.line_height_dip - layout.scroll_y_dip;
            if y + layout.line_height_dip < ry || y > ry + rh {
                continue;
            }
            let line = rope.line(line_idx as usize);
            // Cap per-line characters so a 100k-column line doesn't
            // build a megabyte-long IDWriteTextLayout. 240 chars is
            // enough to read a heading or a code line at minimap scale;
            // beyond that the glyphs would visually merge anyway.
            let mut wide: Vec<u16> = Vec::with_capacity(96);
            'line: for ch in line.chars() {
                if ch == '\n' || ch == '\r' {
                    break;
                }
                let mut buf = [0u16; 2];
                let encoded = ch.encode_utf16(&mut buf);
                for unit in encoded.iter() {
                    wide.push(*unit);
                    if wide.len() >= 240 {
                        break 'line;
                    }
                }
            }
            if wide.is_empty() {
                continue;
            }
            let text_layout = unsafe {
                factory.CreateTextLayout(&wide, &format, text_width, layout.line_height_dip)
            };
            let text_layout = match text_layout {
                Ok(l) => l,
                Err(_) => continue,
            };
            unsafe {
                ctx.DrawTextLayout(
                    D2D_POINT_2F { x: text_left, y },
                    &text_layout,
                    &fg_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }
        }

        unsafe { ctx.SetAntialiasMode(prior_mode) };
    }

    unsafe {
        ctx.PopAxisAlignedClip();
    }

    // Viewport indicator on top of the strip + glyphs.
    let (ix, iy, iw, ih) = layout.indicator_rect;
    if iw > 0.0 && ih > 0.0 {
        let rect = D2D_RECT_F {
            left: ix,
            top: iy,
            right: ix + iw,
            bottom: iy + ih,
        };
        unsafe { ctx.FillRectangle(&rect, &indicator_brush) };
    }

    Ok(())
}

/// Build a minimap-sized text format by copying the editor's font
/// family off `base_format` and constructing a fresh
/// [`IDWriteTextFormat`] at [`MINIMAP_FONT_SIZE_DIP`]. Keeps the
/// minimap visually consistent with the body font without paying for a
/// cached layout per line.
fn build_minimap_format(
    factory: &IDWriteFactory,
    base_format: &IDWriteTextFormat,
) -> Result<IDWriteTextFormat, Error> {
    let mut name_buf = [0u16; 128];
    unsafe {
        base_format.GetFontFamilyName(&mut name_buf)?;
    }
    let len = name_buf
        .iter()
        .position(|&u| u == 0)
        .unwrap_or(name_buf.len());
    let family_w = &name_buf[..len];
    // `locale_name` is `"en-us"` per the rest of the renderer; the
    // minimap doesn't need locale-aware shaping at 2.4-DIP scale.
    let locale_w: Vec<u16> = "en-us".encode_utf16().chain(std::iter::once(0)).collect();
    let format = unsafe {
        factory.CreateTextFormat(
            windows::core::PCWSTR(family_w.as_ptr()),
            None,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            MINIMAP_FONT_SIZE_DIP,
            windows::core::PCWSTR(locale_w.as_ptr()),
        )?
    };
    let _ = MINIMAP_LINE_HEIGHT_DIP;
    Ok(format)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::minimap::compute_minimap_layout;
    use crate::params::Rgba;

    /// The painter needs an `ID2D1DeviceContext`, which can't be built
    /// in a unit test. Coverage of the visible output lives in the
    /// pixel canary (`minimap_scaled` fixture); here we just verify
    /// that the layout the painter consumes is well-formed for a small
    /// buffer and that constants are wired through.
    #[test]
    fn layout_is_consumed_well_formed() {
        let l = compute_minimap_layout((0.0, 0.0, 800.0, 600.0), 0.0, 18.0, 5, 5.0 * 18.0, 0.0);
        assert!(l.rect.2 > 0.0);
        assert_eq!(l.line_height_dip, MINIMAP_LINE_HEIGHT_DIP);
        assert_eq!(l.font_size_dip, MINIMAP_FONT_SIZE_DIP);
        let _ = MinimapColors {
            bg: Rgba::default(),
            fg: Rgba::default(),
            viewport_indicator: Rgba::default(),
        };
    }
}
