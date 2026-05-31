//! Phase F2 — outline-sidebar D2D paint dispatch.
//!
//! Pure pure-layout-plus-paint sibling to [`crate::outline`]: takes the
//! `OutlineData` the UI built this frame, computes an [`OutlineLayout`]
//! against the focused pane's body rect, fills the strip, draws a
//! one-DIP separator on the left edge, and renders one row of heading
//! text per visible entry. The row whose index matches
//! `data.current_index` paints with the `foreground_active` color.
//!
//! The painter returns the freshly-computed [`OutlineLayout`] so the UI
//! orchestrator can cache it for the next mouse click's hit-test. Paint
//! failures are non-fatal (mirrors [`crate::status_bar::paint_status_bar`])
//! — the sidebar is decoration, never an editor blocker.
//!
//! Thread ownership: render thread of the owning window (caller).

use windows::Win32::Graphics::Direct2D::Common::{D2D_POINT_2F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_ALIASED,
    D2D1_DRAW_TEXT_OPTIONS_NONE,
};
use windows::Win32::Graphics::DirectWrite::DWRITE_WORD_WRAPPING_NO_WRAP;
use windows::Win32::Graphics::DirectWrite::{IDWriteFactory, IDWriteTextFormat};

use crate::outline::{
    compute_outline_layout, compute_outline_scroll_indicator, indent_for_level, OutlineData,
    OutlineLayout, OUTLINE_ROW_HEIGHT_DIP,
};
use crate::params::{DrawParams, Rgba};
use crate::Error;

/// Paint the outline sidebar into the focused pane's body rect.
///
/// `pane_rect` is the focused pane's body rect in client DIPs. The
/// sidebar docks to its right edge at `data.width_dip` wide. Returns the
/// layout produced so the UI can cache it for the next click hit-test.
///
/// # Errors
///
/// Returns [`Error::Win`] on a DirectWrite text-layout creation failure.
#[allow(clippy::too_many_arguments)]
pub fn paint_outline(
    ctx: &ID2D1DeviceContext,
    factory: &IDWriteFactory,
    format: &IDWriteTextFormat,
    data: &OutlineData<'_>,
    pane_rect: (f32, f32, f32, f32),
    bg_brush: &ID2D1SolidColorBrush,
    fg_brush: &ID2D1SolidColorBrush,
    fg_active_brush: &ID2D1SolidColorBrush,
    separator_brush: &ID2D1SolidColorBrush,
) -> Result<OutlineLayout, Error> {
    let layout = paint_outline_shell(ctx, data, pane_rect, bg_brush, separator_brush);
    paint_outline_entries(
        ctx,
        factory,
        format,
        data,
        &layout,
        fg_brush,
        fg_active_brush,
    )?;
    Ok(layout)
}

/// Paint only the retained outline-sidebar shell.
pub(crate) fn paint_outline_shell(
    ctx: &ID2D1DeviceContext,
    data: &OutlineData<'_>,
    pane_rect: (f32, f32, f32, f32),
    bg_brush: &ID2D1SolidColorBrush,
    separator_brush: &ID2D1SolidColorBrush,
) -> OutlineLayout {
    let layout = compute_outline_layout(data, pane_rect, data.scroll_offset_dip);
    let (rx, ry, rw, rh) = layout.rect;
    if rw <= 0.0 || rh <= 0.0 {
        return layout;
    }
    let bar = D2D_RECT_F {
        left: rx,
        top: ry,
        right: rx + rw,
        bottom: ry + rh,
    };
    unsafe { ctx.FillRectangle(&bar, bg_brush) };
    let sep = D2D_RECT_F {
        left: rx,
        top: ry,
        right: rx + 1.0,
        bottom: ry + rh,
    };
    unsafe { ctx.FillRectangle(&sep, separator_brush) };
    layout
}

fn paint_outline_entries(
    ctx: &ID2D1DeviceContext,
    factory: &IDWriteFactory,
    format: &IDWriteTextFormat,
    data: &OutlineData<'_>,
    layout: &OutlineLayout,
    fg_brush: &ID2D1SolidColorBrush,
    fg_active_brush: &ID2D1SolidColorBrush,
) -> Result<(), Error> {
    let (rx, ry, rw, rh) = layout.rect;
    if rw <= 0.0 || rh <= 0.0 {
        return Ok(());
    }
    let clip = D2D_RECT_F {
        left: rx,
        top: ry,
        right: rx + rw,
        bottom: ry + rh,
    };
    unsafe {
        ctx.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED);
    }
    let result = paint_outline_entry_rows(
        ctx,
        factory,
        format,
        data,
        layout,
        fg_brush,
        fg_active_brush,
    );
    unsafe {
        ctx.PopAxisAlignedClip();
    }
    result?;
    paint_outline_scrollbar(ctx, layout, fg_active_brush);
    Ok(())
}

fn paint_outline_entry_rows(
    ctx: &ID2D1DeviceContext,
    factory: &IDWriteFactory,
    format: &IDWriteTextFormat,
    data: &OutlineData<'_>,
    layout: &OutlineLayout,
    fg_brush: &ID2D1SolidColorBrush,
    fg_active_brush: &ID2D1SolidColorBrush,
) -> Result<(), Error> {
    let (rx, _, rw, _) = layout.rect;
    let current_index = data.current_index;
    for row in &layout.rows {
        let entry = match data.entries.get(row.entry_index as usize) {
            Some(e) => e,
            None => continue,
        };
        let indent = indent_for_level(entry.level);
        let left = rx + indent;
        let avail = (rx + rw - left - 4.0).max(1.0);
        let brush = if Some(row.entry_index) == current_index {
            fg_active_brush
        } else {
            fg_brush
        };
        let wide: Vec<u16> = entry.text.encode_utf16().collect();
        let text_layout =
            unsafe { factory.CreateTextLayout(&wide, format, avail, OUTLINE_ROW_HEIGHT_DIP)? };
        unsafe {
            text_layout.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        }
        unsafe {
            ctx.DrawTextLayout(
                D2D_POINT_2F {
                    x: left,
                    y: row.top,
                },
                &text_layout,
                brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }
    }
    Ok(())
}

fn paint_outline_scrollbar(
    ctx: &ID2D1DeviceContext,
    layout: &OutlineLayout,
    brush: &ID2D1SolidColorBrush,
) {
    let Some((x, y, w, h)) = compute_outline_scroll_indicator(layout) else {
        return;
    };
    let rect = D2D_RECT_F {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    };
    unsafe { ctx.FillRectangle(&rect, brush) };
}

/// Convenience wrapper for the per-frame outline text pass. The shell
/// fill and separator are retained in the static chrome command list.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_outline_frame_text<F>(
    ctx: &ID2D1DeviceContext,
    factory: &IDWriteFactory,
    format: &IDWriteTextFormat,
    data: &OutlineData<'_>,
    pane_rect: (f32, f32, f32, f32),
    make_brush: &F,
) -> Option<OutlineLayout>
where
    F: Fn(Rgba) -> Result<ID2D1SolidColorBrush, Error>,
{
    let foreground_brush = make_brush(data.colors.fg).ok()?;
    let foreground_active_brush = make_brush(data.colors.fg_active).ok()?;
    let layout = compute_outline_layout(data, pane_rect, data.scroll_offset_dip);
    paint_outline_entries(
        ctx,
        factory,
        format,
        data,
        &layout,
        &foreground_brush,
        &foreground_active_brush,
    )
    .ok()?;
    Some(layout)
}

/// Renderer dispatch helper for only the dynamic outline entry text.
pub(crate) fn dispatch_outline_text_paint<F>(
    ctx: &ID2D1DeviceContext,
    factory: &IDWriteFactory,
    params: &DrawParams<'_>,
    viewport_w: f32,
    viewport_h: f32,
    make_brush: &F,
) where
    F: Fn(Rgba) -> Result<ID2D1SolidColorBrush, Error>,
{
    if !params.view_options.show_outline_sidebar {
        return;
    }
    let Some(data) = params.outline else { return };
    let pane_rect = (
        params.body_origin.0,
        params.body_origin.1,
        viewport_w,
        viewport_h,
    );
    let _ = paint_outline_frame_text(ctx, factory, params.format, data, pane_rect, make_brush);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outline::{OutlineColors, OutlineEntry, OUTLINE_DEFAULT_WIDTH_DIP};

    fn entry(text: &str, level: u8, target: u32) -> OutlineEntry {
        OutlineEntry {
            text: text.into(),
            level,
            target_byte: target,
        }
    }

    /// The `current_index` field on `OutlineData` is the only piece of
    /// state the paint pass consumes beyond the entries themselves.
    /// Pure-layout coverage of the paint contract: the row whose index
    /// matches `current_index` ends up at the same layout position the
    /// painter will route to `fg_active_brush`.
    #[test]
    fn paint_dispatch_uses_current_index_for_active_row_lookup() {
        let entries = vec![
            entry("Top", 1, 0),
            entry("Sub", 2, 10),
            entry("Deep", 3, 24),
        ];
        let data = OutlineData {
            entries: &entries,
            current_index: Some(1),
            colors: OutlineColors::default(),
            width_dip: OUTLINE_DEFAULT_WIDTH_DIP,
            font_size_dip: 14.0,
            scroll_offset_dip: 0.0,
        };
        let layout = crate::outline::compute_outline_layout(&data, (0.0, 0.0, 800.0, 600.0), 0.0);
        // The row whose `entry_index` matches `current_index` is the
        // one the painter binds to the active brush.
        let active = layout
            .rows
            .iter()
            .find(|r| Some(r.entry_index) == data.current_index)
            .expect("active row present in layout");
        assert_eq!(active.entry_index, 1);
    }
}
