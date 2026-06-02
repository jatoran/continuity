//! Overlay primitives for find-bar / palette / quick-open / goto panels.
//!
//! All draw helpers run *inside* an existing `BeginDraw` / `EndDraw` scope on
//! the renderer's `ID2D1DeviceContext`. The renderer's `draw_buffer` calls
//! [`paint_overlay`] after painting the editor body and before the final
//! `Present`.
//!
//! No allocation per-frame beyond the temporary brushes; brushes are created
//! per call (cheap relative to `Present`) to avoid threading lifetime
//! constraints through the public types.

use windows::core::Interface;
use windows::Win32::Graphics::Direct2D::Common::{D2D_POINT_2F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1RenderTarget, ID2D1SolidColorBrush, D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_ROUNDED_RECT,
};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory, IDWriteTextFormat, DWRITE_HIT_TEST_METRICS, DWRITE_TEXT_RANGE,
};

use crate::{Error, Rgba};

pub(crate) mod brush;
#[cfg(test)]
mod tests;

use brush::BrushCache;

/// A device-independent rectangle in DIPs.
#[derive(Copy, Clone, Debug, Default)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

impl Rect {
    /// Construct a rect from (x, y, w, h).
    #[must_use]
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    fn to_d2d(self) -> D2D_RECT_F {
        D2D_RECT_F {
            left: self.x,
            top: self.y,
            right: self.x + self.w,
            bottom: self.y + self.h,
        }
    }

    /// Return a rectangle inset by `dx`/`dy`, clamping width/height at zero.
    #[must_use]
    pub fn inset(self, dx: f32, dy: f32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            w: (self.w - 2.0 * dx).max(0.0),
            h: (self.h - 2.0 * dy).max(0.0),
        }
    }

    /// Return a rectangle translated by `dx`/`dy`.
    #[must_use]
    pub fn translate(self, dx: f32, dy: f32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            w: self.w,
            h: self.h,
        }
    }
}

/// Visual style for the panel that backs every overlay.
#[derive(Clone, Debug)]
pub struct PanelStyle {
    /// Outer panel bounds.
    pub rect: Rect,
    /// Corner radius (DIPs).
    pub corner_radius: f32,
    /// Panel fill.
    pub bg: Rgba,
    /// 1-DIP stroke around the panel.
    pub border: Rgba,
    /// Soft drop shadow color (alpha typically 0.3..=0.5).
    pub shadow: Rgba,
    /// Drop-shadow offset.
    pub shadow_offset: f32,
}

/// A focused single-line text input drawn inside a panel.
#[derive(Clone, Debug)]
pub struct FocusField {
    /// Field bounds (inside the panel).
    pub rect: Rect,
    /// Current input text.
    pub text: String,
    /// Placeholder text drawn when `text` is empty.
    pub placeholder: Option<String>,
    /// Byte offset (within `text`) where the caret is rendered.
    pub caret_byte: usize,
    /// Ordered byte range selected inside `text`, when non-empty.
    pub selection_range: Option<(usize, usize)>,
    /// Field text color.
    pub fg: Rgba,
    /// Selection highlight fill.
    pub selection_bg: Rgba,
    /// Placeholder color.
    pub placeholder_fg: Rgba,
    /// Caret color.
    pub caret_color: Rgba,
    /// 1-DIP focus ring color.
    pub focus_ring: Rgba,
}

/// A single row in a list overlay (palette result, quick-open candidate,…).
#[derive(Clone, Debug)]
pub struct ListRow {
    /// Row bounds.
    pub rect: Rect,
    /// Main text.
    pub primary_text: String,
    /// Secondary text drawn after `primary_text`, in dimmer color.
    pub secondary_text: Option<String>,
    /// Right-aligned keybinding hint.
    pub keybinding: Option<String>,
    /// Primary text color.
    pub fg: Rgba,
    /// Secondary text color (also used for `keybinding`).
    pub secondary_fg: Rgba,
    /// Selection / hover background; `None` for unhighlighted rows.
    pub bg: Option<Rgba>,
    /// `true` for predicate-grayed-out rows; primary text is dimmed.
    pub disabled: bool,
}

/// Vertical scrollbar for a list overlay.
#[derive(Clone, Debug)]
pub struct OverlayScrollbar {
    /// Full scrollbar track bounds.
    pub track: Rect,
    /// Scroll thumb bounds within the track.
    pub thumb: Rect,
    /// Track fill color.
    pub track_color: Rgba,
    /// Thumb fill color.
    pub thumb_color: Rgba,
}

/// One overlay's complete paint state.
#[derive(Clone, Debug)]
pub struct OverlayDraw {
    /// Backing panel.
    pub panel: PanelStyle,
    /// `true` when the overlay text input currently owns keyboard focus.
    pub input_focused: bool,
    /// Optional input field — the one currently holding caret focus.
    pub focus_field: Option<FocusField>,
    /// Optional second input field. Painted *without* a caret or
    /// focus ring so the user sees both inputs but knows which is
    /// active. Used by the G4-UX dual-input find/replace bar.
    pub secondary_field: Option<FocusField>,
    /// List rows, drawn top-down in order.
    pub list_rows: Vec<ListRow>,
    /// Optional vertical scrollbar for overflowed list rows.
    pub scrollbar: Option<OverlayScrollbar>,
    /// Optional footer text, drawn left-aligned in `secondary_fg`.
    pub footer: Option<FooterText>,
}

/// Footer caption (e.g. "23 of 412") drawn inside the panel.
#[derive(Clone, Debug)]
pub struct FooterText {
    /// Footer bounds.
    pub rect: Rect,
    /// Footer text.
    pub text: String,
    /// Footer color.
    pub fg: Rgba,
}

/// Paint `overlay` onto `ctx`. Must be called inside an active
/// `BeginDraw`/`EndDraw` bracket. Returns [`Error::Graphics`] on Win32 failure.
///
/// # Errors
///
/// Returns [`Error::Graphics`] on any underlying Win32 failure.
pub fn paint_overlay(
    ctx: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    overlay: &OverlayDraw,
    chrome_font_size_dip: f32,
) -> Result<(), Error> {
    let render_target: ID2D1RenderTarget = ctx.cast()?;
    let mut brushes = BrushCache::new(&render_target)?;
    paint_panel(ctx, &mut brushes, &overlay.panel)?;
    if let Some(field) = &overlay.focus_field {
        paint_focus_field(
            ctx,
            dwrite,
            format,
            &mut brushes,
            field,
            overlay.input_focused,
            chrome_font_size_dip,
        )?;
    }
    if let Some(field) = &overlay.secondary_field {
        // Secondary fields paint without a caret + focus ring so the
        // user can see both inputs at once but knows which is active.
        paint_focus_field(
            ctx,
            dwrite,
            format,
            &mut brushes,
            field,
            false,
            chrome_font_size_dip,
        )?;
    }
    for row in &overlay.list_rows {
        paint_list_row(ctx, dwrite, &mut brushes, format, row, chrome_font_size_dip)?;
    }
    if let Some(scrollbar) = &overlay.scrollbar {
        crate::overlay_scrollbar::paint_overlay_scrollbar(ctx, &mut brushes, scrollbar)?;
    }
    if let Some(footer) = &overlay.footer {
        paint_footer(
            ctx,
            dwrite,
            &mut brushes,
            format,
            footer,
            chrome_font_size_dip,
        )?;
    }
    Ok(())
}

/// Draw `text` inside `rect` at a pinned `font_size_dip`, regardless of the
/// size baked into `format`. Overlays are chrome: their layout rects are
/// fixed (e.g. `overlay_render::ROW_HEIGHT`), so the text must stay a fixed
/// size or it overflows the row when the body `format` is zoomed up. Mirrors
/// the status-bar `SetFontSize` technique. `font_size_dip <= 0` leaves the
/// format's own size.
fn draw_text_sized(
    ctx: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    text: &str,
    rect: D2D_RECT_F,
    font_size_dip: f32,
    brush: &ID2D1SolidColorBrush,
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
        ctx.DrawTextLayout(
            D2D_POINT_2F {
                x: rect.left,
                y: rect.top,
            },
            &layout,
            brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
        );
    }
    Ok(())
}

fn paint_panel(
    ctx: &ID2D1DeviceContext,
    brushes: &mut BrushCache,
    panel: &PanelStyle,
) -> Result<(), Error> {
    if panel.shadow.a > 0.0 {
        let shadow_brush = brushes.solid(panel.shadow)?;
        let shadow_rect = panel
            .rect
            .translate(panel.shadow_offset, panel.shadow_offset);
        let rr = D2D1_ROUNDED_RECT {
            rect: shadow_rect.to_d2d(),
            radiusX: panel.corner_radius,
            radiusY: panel.corner_radius,
        };
        unsafe {
            ctx.FillRoundedRectangle(&rr, &shadow_brush);
        }
    }
    let bg_brush = brushes.solid(panel.bg)?;
    let rr = D2D1_ROUNDED_RECT {
        rect: panel.rect.to_d2d(),
        radiusX: panel.corner_radius,
        radiusY: panel.corner_radius,
    };
    unsafe {
        ctx.FillRoundedRectangle(&rr, &bg_brush);
    }
    if panel.border.a > 0.0 {
        let border_brush = brushes.solid(panel.border)?;
        unsafe {
            ctx.DrawRoundedRectangle(&rr, &border_brush, 1.0, None);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn paint_focus_field(
    ctx: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    brushes: &mut BrushCache,
    field: &FocusField,
    is_focused: bool,
    font_size_dip: f32,
) -> Result<(), Error> {
    if is_focused {
        let ring = D2D1_ROUNDED_RECT {
            rect: field.rect.inset(-1.0, -1.0).to_d2d(),
            radiusX: 4.0,
            radiusY: 4.0,
        };
        let ring_brush = brushes.solid(field.focus_ring)?;
        unsafe {
            ctx.DrawRoundedRectangle(&ring, &ring_brush, 1.0, None);
        }
    }

    let inner = field.rect.inset(8.0, 4.0);
    let display: &str = if field.text.is_empty() {
        field.placeholder.as_deref().unwrap_or("")
    } else {
        field.text.as_str()
    };
    let display_color = if field.text.is_empty() {
        field.placeholder_fg
    } else {
        field.fg
    };
    if !display.is_empty() {
        if !field.text.is_empty() {
            paint_focus_field_selection(dwrite, format, brushes, ctx, field, inner, font_size_dip)?;
        }
        let brush = brushes.solid(display_color)?;
        draw_text_sized(
            ctx,
            dwrite,
            format,
            display,
            inner.to_d2d(),
            font_size_dip,
            &brush,
        )?;
    }
    if is_focused {
        let caret_x = if field.text.is_empty() {
            0.0
        } else {
            caret_offset_in_field(dwrite, format, &field.text, field.caret_byte, font_size_dip)
                .unwrap_or(0.0)
        };
        let caret_brush = brushes.solid(field.caret_color)?;
        let caret_rect = D2D_RECT_F {
            left: inner.x + caret_x,
            top: inner.y,
            right: inner.x + caret_x + 1.5,
            bottom: inner.y + inner.h,
        };
        unsafe {
            ctx.FillRectangle(&caret_rect, &caret_brush);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn paint_focus_field_selection(
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    brushes: &mut BrushCache,
    ctx: &ID2D1DeviceContext,
    field: &FocusField,
    inner: Rect,
    font_size_dip: f32,
) -> Result<(), Error> {
    let Some((start, end)) = normalized_selection_range(&field.text, field.selection_range) else {
        return Ok(());
    };
    let left =
        caret_offset_in_field(dwrite, format, &field.text, start, font_size_dip).unwrap_or(0.0);
    let right =
        caret_offset_in_field(dwrite, format, &field.text, end, font_size_dip).unwrap_or(left);
    if right <= left {
        return Ok(());
    }
    let rect = D2D_RECT_F {
        left: inner.x + left,
        top: inner.y,
        right: inner.x + right,
        bottom: inner.y + inner.h,
    };
    let brush = brushes.solid(field.selection_bg)?;
    unsafe {
        ctx.FillRectangle(&rect, &brush);
    }
    Ok(())
}

fn normalized_selection_range(
    text: &str,
    selection_range: Option<(usize, usize)>,
) -> Option<(usize, usize)> {
    let (mut start, mut end) = selection_range?;
    if end < start {
        std::mem::swap(&mut start, &mut end);
    }
    start = previous_char_boundary(text, start.min(text.len()));
    end = previous_char_boundary(text, end.min(text.len()));
    (start < end).then_some((start, end))
}

fn previous_char_boundary(text: &str, byte: usize) -> usize {
    let mut clamped = byte.min(text.len());
    while clamped > 0 && !text.is_char_boundary(clamped) {
        clamped -= 1;
    }
    clamped
}

fn paint_list_row(
    ctx: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    brushes: &mut BrushCache,
    format: &IDWriteTextFormat,
    row: &ListRow,
    font_size_dip: f32,
) -> Result<(), Error> {
    if let Some(bg) = row.bg {
        let brush = brushes.solid(bg)?;
        unsafe {
            ctx.FillRectangle(&row.rect.to_d2d(), &brush);
        }
    }
    let text_rect = row.rect.inset(8.0, 2.0);
    let primary_color = if row.disabled {
        Rgba {
            a: row.fg.a * 0.5,
            ..row.fg
        }
    } else {
        row.fg
    };
    if !row.primary_text.is_empty() {
        let brush = brushes.solid(primary_color)?;
        draw_text_sized(
            ctx,
            dwrite,
            format,
            &row.primary_text,
            text_rect.to_d2d(),
            font_size_dip,
            &brush,
        )?;
    }
    if let Some(secondary) = row.secondary_text.as_deref() {
        let brush = brushes.solid(row.secondary_fg)?;
        let mut secondary_rect = text_rect;
        secondary_rect.x = text_rect.x + 0.55 * text_rect.w;
        secondary_rect.w = 0.45 * text_rect.w;
        draw_text_sized(
            ctx,
            dwrite,
            format,
            secondary,
            secondary_rect.to_d2d(),
            font_size_dip,
            &brush,
        )?;
    }
    if let Some(kb) = row.keybinding.as_deref() {
        let brush = brushes.solid(row.secondary_fg)?;
        let kb_w = (kb.chars().count() as f32) * 8.0 + 16.0;
        let kb_rect = D2D_RECT_F {
            left: row.rect.x + row.rect.w - kb_w,
            top: text_rect.y,
            right: row.rect.x + row.rect.w - 4.0,
            bottom: text_rect.y + text_rect.h,
        };
        draw_text_sized(ctx, dwrite, format, kb, kb_rect, font_size_dip, &brush)?;
    }
    Ok(())
}

fn paint_footer(
    ctx: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    brushes: &mut BrushCache,
    format: &IDWriteTextFormat,
    footer: &FooterText,
    font_size_dip: f32,
) -> Result<(), Error> {
    if footer.text.is_empty() {
        return Ok(());
    }
    let brush = brushes.solid(footer.fg)?;
    draw_text_sized(
        ctx,
        dwrite,
        format,
        &footer.text,
        footer.rect.inset(8.0, 2.0).to_d2d(),
        font_size_dip,
        &brush,
    )?;
    Ok(())
}

fn caret_offset_in_field(
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

fn utf8_byte_to_utf16_index(s: &str, byte_in_line: usize) -> usize {
    if byte_in_line >= s.len() {
        return s.encode_utf16().count();
    }
    let mut consumed = 0;
    let mut idx = 0;
    for ch in s.chars() {
        let n = ch.len_utf8();
        if consumed + n > byte_in_line {
            break;
        }
        consumed += n;
        idx += ch.len_utf16();
    }
    idx
}
