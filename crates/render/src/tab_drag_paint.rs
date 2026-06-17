//! In-flight tab-drag visual feedback painter.
//!
//! Renders the four affordances surfaced by [`crate::TabDragOverlayDraw`]:
//! - 2-DIP accent insertion bar between tabs on a tab strip the cursor
//!   is currently over (source pane, sibling pane, or foreign window).
//! - Source-tab fade — overlays a translucent strip background over
//!   the lifted tab so the user reads it as "in the cursor."
//! - Pane-body drop highlight — 2-DIP accent border + 6 % accent tint
//!   on the target pane body when a cross-pane drop is the active
//!   resolution.
//! - Cursor-attached tear-off ghost — small rectangle near the cursor
//!   when the cursor sits in the tear-off zone.
//!
//! No theme keys are added; the affordance shares the same accent the
//! focused pane's border already uses (`pane.border_active`) so the
//! drop feedback is theme-aware without expanding the bundled theme
//! contract.

use windows::core::{Interface, HSTRING};
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_POINT_2F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1SolidColorBrush, D2D1_DRAW_TEXT_OPTIONS_NONE,
};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_REGULAR, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
};

use crate::pane_chrome_layout::tab_slot_widths;
use crate::params::{
    PaneBodyDropHighlight, PaneChromeDraw, Rgba, TabDragGhostDraw, TabDragSourceFade,
    TabStripInsertionBarDraw,
};
use crate::Error;

/// Body-drop highlight tint alpha — the 6 % accent overlay described
/// in the task. Picked to read as a hint rather than steal contrast
/// from the underlying text.
const PANE_BODY_TINT_ALPHA: f32 = 0.06;
/// Inset from each edge of the pane body for the highlight border so
/// it doesn't fight the pane focus chrome (which lives flush with the
/// outer rect).
const PANE_BODY_INSET_DIP: f32 = 4.0;
/// Border thickness for the pane-body highlight in DIPs.
const PANE_BODY_BORDER_DIP: f32 = 2.0;
/// Ghost overall opacity. Sub-1.0 so the surface behind reads through.
const GHOST_OVERALL_ALPHA: f32 = 0.8;
/// Ghost border thickness in DIPs.
const GHOST_BORDER_DIP: f32 = 1.5;
/// Ghost rounded corner radius. Subtle — matches the strip's softer
/// aesthetic without competing with the rest of the chrome.
const GHOST_LABEL_PADDING_DIP: f32 = 10.0;
/// Ghost label font size in DIPs.
const GHOST_FONT_SIZE_DIP: f32 = 12.5;

/// Paint the in-flight tab-drag affordance, if any.
///
/// Runs *after* `paint_pane_chrome` so the insertion bar and pane-body
/// highlight overlay the strip / body the chrome pass just drew. The
/// ghost paints last so it stays on top of everything.
pub fn paint_tab_drag_overlay(
    d2d: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    chrome: &PaneChromeDraw,
    font_family: &str,
    locale: &str,
) -> Result<(), Error> {
    let Some(overlay) = chrome.tab_drag.as_ref() else {
        return Ok(());
    };
    let alpha = overlay.fade_alpha.clamp(0.0, 1.0);
    if alpha <= 0.001 {
        return Ok(());
    }
    let accent = chrome.colors.pane_border_active;
    unsafe {
        let render_target: windows::Win32::Graphics::Direct2D::ID2D1RenderTarget = d2d.cast()?;
        if let Some(fade) = overlay.source_tab.as_ref() {
            paint_source_tab_fade(d2d, &render_target, chrome, fade, alpha)?;
        }
        if let Some(highlight) = overlay.pane_body_highlight.as_ref() {
            paint_pane_body_highlight(d2d, &render_target, highlight, accent, alpha)?;
        }
        if let Some(bar) = overlay.source_strip_indicator.as_ref() {
            paint_insertion_bar(d2d, &render_target, bar, accent, alpha)?;
        }
        if let Some(ghost) = overlay.ghost.as_ref() {
            paint_tear_off_ghost(
                d2d,
                dwrite,
                &render_target,
                ghost,
                chrome,
                accent,
                font_family,
                locale,
                alpha,
            )?;
        }
    }
    Ok(())
}

unsafe fn paint_source_tab_fade(
    d2d: &ID2D1DeviceContext,
    render_target: &windows::Win32::Graphics::Direct2D::ID2D1RenderTarget,
    chrome: &PaneChromeDraw,
    fade: &TabDragSourceFade,
    alpha: f32,
) -> Result<(), Error> {
    // Compute where the source tab sits inside the strip — same width
    // algorithm `paint_one_pane_strip` uses so the fade overlay lands
    // exactly on the tab the user grabbed.
    let pane = chrome.panes.iter().find(|p| {
        (p.outer.0 - fade.strip_outer.0).abs() < 0.5 && (p.outer.1 - fade.strip_outer.1).abs() < 0.5
    });
    let Some(pane) = pane else {
        return Ok(());
    };
    if fade.tab_index >= pane.tabs.len() {
        return Ok(());
    }
    let labels: Vec<&str> = pane.tabs.iter().map(|t| t.text.as_ref()).collect();
    let widths = tab_slot_widths(&labels, fade.strip_outer.2);
    let tab_x: f32 = widths.iter().take(fade.tab_index).sum();
    let tab_w = widths.get(fade.tab_index).copied().unwrap_or(0.0);
    if tab_w <= 0.0 {
        return Ok(());
    }
    let (sx, sy, _, _) = fade.strip_outer;
    // Overlay the strip background over the tab to fade the painted
    // tab + label down. Alpha = (1 - target_alpha) * fade_alpha so a
    // 0.6-target with a half-step fade picks 0.2 — the eye reads the
    // tab as ~70 % opaque, the design intent.
    let fade_drop = (1.0 - fade.alpha).clamp(0.0, 1.0);
    let overlay_alpha = fade_drop * alpha;
    if overlay_alpha <= 0.001 {
        return Ok(());
    }
    let strip_bg = with_alpha(chrome.colors.bg, overlay_alpha);
    let color: D2D1_COLOR_F = strip_bg.into();
    let brush: ID2D1SolidColorBrush = render_target.CreateSolidColorBrush(&color, None)?;
    let rect = D2D_RECT_F {
        left: sx + tab_x,
        top: sy,
        right: sx + tab_x + tab_w,
        bottom: sy + chrome.strip_height.min(fade.strip_outer.3),
    };
    d2d.FillRectangle(&rect, &brush);
    Ok(())
}

unsafe fn paint_insertion_bar(
    d2d: &ID2D1DeviceContext,
    render_target: &windows::Win32::Graphics::Direct2D::ID2D1RenderTarget,
    bar: &TabStripInsertionBarDraw,
    accent: Rgba,
    alpha: f32,
) -> Result<(), Error> {
    let (sx, sy, _, _) = bar.strip_outer;
    let color: D2D1_COLOR_F = with_alpha(accent, alpha).into();
    let brush: ID2D1SolidColorBrush = render_target.CreateSolidColorBrush(&color, None)?;
    let left = sx + bar.x_in_strip;
    let rect = D2D_RECT_F {
        left,
        top: sy,
        right: left + bar.width,
        bottom: sy + bar.height,
    };
    d2d.FillRectangle(&rect, &brush);
    Ok(())
}

unsafe fn paint_pane_body_highlight(
    d2d: &ID2D1DeviceContext,
    render_target: &windows::Win32::Graphics::Direct2D::ID2D1RenderTarget,
    highlight: &PaneBodyDropHighlight,
    accent: Rgba,
    alpha: f32,
) -> Result<(), Error> {
    let (bx, by, bw, bh) = highlight.body_rect;
    if bw <= PANE_BODY_INSET_DIP * 2.0 || bh <= PANE_BODY_INSET_DIP * 2.0 {
        return Ok(());
    }
    let inset_rect = D2D_RECT_F {
        left: bx + PANE_BODY_INSET_DIP,
        top: by + PANE_BODY_INSET_DIP,
        right: bx + bw - PANE_BODY_INSET_DIP,
        bottom: by + bh - PANE_BODY_INSET_DIP,
    };
    // 6 % accent tint over the body.
    let tint: D2D1_COLOR_F = with_alpha(accent, PANE_BODY_TINT_ALPHA * alpha).into();
    let tint_brush: ID2D1SolidColorBrush = render_target.CreateSolidColorBrush(&tint, None)?;
    d2d.FillRectangle(&inset_rect, &tint_brush);
    // 2 DIP accent border.
    let border_color: D2D1_COLOR_F = with_alpha(accent, alpha).into();
    let border_brush: ID2D1SolidColorBrush =
        render_target.CreateSolidColorBrush(&border_color, None)?;
    d2d.DrawRectangle(&inset_rect, &border_brush, PANE_BODY_BORDER_DIP, None);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
unsafe fn paint_tear_off_ghost(
    d2d: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    render_target: &windows::Win32::Graphics::Direct2D::ID2D1RenderTarget,
    ghost: &TabDragGhostDraw,
    chrome: &PaneChromeDraw,
    accent: Rgba,
    font_family: &str,
    locale: &str,
    alpha: f32,
) -> Result<(), Error> {
    let overall = alpha * GHOST_OVERALL_ALPHA;
    if overall <= 0.001 || ghost.width <= 0.0 || ghost.height <= 0.0 {
        return Ok(());
    }
    let rect = D2D_RECT_F {
        left: ghost.origin.0,
        top: ghost.origin.1,
        right: ghost.origin.0 + ghost.width,
        bottom: ghost.origin.1 + ghost.height,
    };
    // Background fill — panel surface so the ghost reads as
    // "this is a tab pulled off." Accent border outlines it.
    let bg: D2D1_COLOR_F = with_alpha(chrome.colors.bg, overall).into();
    let bg_brush: ID2D1SolidColorBrush = render_target.CreateSolidColorBrush(&bg, None)?;
    d2d.FillRectangle(&rect, &bg_brush);
    let border: D2D1_COLOR_F = with_alpha(accent, overall).into();
    let border_brush: ID2D1SolidColorBrush = render_target.CreateSolidColorBrush(&border, None)?;
    d2d.DrawRectangle(&rect, &border_brush, GHOST_BORDER_DIP, None);
    // Label text.
    let fg: D2D1_COLOR_F = with_alpha(chrome.colors.fg, overall).into();
    let fg_brush: ID2D1SolidColorBrush = render_target.CreateSolidColorBrush(&fg, None)?;
    let family = HSTRING::from(font_family);
    let loc = HSTRING::from(locale);
    let format = dwrite.CreateTextFormat(
        &family,
        None,
        DWRITE_FONT_WEIGHT_REGULAR,
        DWRITE_FONT_STYLE_NORMAL,
        DWRITE_FONT_STRETCH_NORMAL,
        GHOST_FONT_SIZE_DIP,
        &loc,
    )?;
    format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
    format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
    let wide: Vec<u16> = ghost.label.encode_utf16().collect();
    let layout_rect = D2D_RECT_F {
        left: rect.left + GHOST_LABEL_PADDING_DIP,
        top: rect.top,
        right: rect.right - GHOST_LABEL_PADDING_DIP,
        bottom: rect.bottom,
    };
    let layout_w = (layout_rect.right - layout_rect.left).max(1.0);
    let layout_h = (layout_rect.bottom - layout_rect.top).max(1.0);
    let layout = dwrite.CreateTextLayout(&wide, &format, layout_w, layout_h)?;
    d2d.DrawTextLayout(
        D2D_POINT_2F {
            x: layout_rect.left,
            y: layout_rect.top,
        },
        &layout,
        &fg_brush,
        D2D1_DRAW_TEXT_OPTIONS_NONE,
    );
    Ok(())
}

fn with_alpha(color: Rgba, alpha: f32) -> Rgba {
    Rgba {
        a: color.a * alpha.clamp(0.0, 1.0),
        ..color
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_multiplication_clamps_to_unit_range() {
        let c = Rgba {
            r: 0.1,
            g: 0.2,
            b: 0.3,
            a: 1.0,
        };
        let half = with_alpha(c, 0.5);
        assert!((half.a - 0.5).abs() < 1e-6);
        let zero = with_alpha(c, -1.0);
        assert!((zero.a - 0.0).abs() < 1e-6);
        let one = with_alpha(c, 2.0);
        assert!((one.a - 1.0).abs() < 1e-6);
    }
}
