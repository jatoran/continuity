//! Phase 13 — paint per-pane tab strips and pane borders.
//!
//! All work happens between an outer `BeginDraw` / `EndDraw` orchestrated
//! by [`crate::Renderer::draw_buffer`]; this module only issues D2D /
//! DirectWrite calls — no swap-chain interaction.

use windows::core::{Interface, HSTRING};
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_POINT_2F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1Factory, ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_ALIASED,
    D2D1_DRAW_TEXT_OPTIONS_CLIP,
};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory, IDWriteTextFormat, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_REGULAR, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_WORD_WRAPPING_NO_WRAP,
};

use crate::pane_chrome_border::paint_pane_border;
use crate::pane_chrome_tabs::{
    close_button_rect, paint_close_icon, paint_tab_background, TAB_TRAPEZOID_SKEW_DIP,
};
use crate::params::{PaneChromeDraw, PaneStripDraw, Rgba};
use crate::Error;

/// Per-tab horizontal padding inside the strip.
pub const TAB_PADDING_DIP: f32 = 12.0;
/// Tab strip font size in DIPs.
pub(crate) const STRIP_FONT_SIZE_DIP: f32 = 12.5;
/// Border thickness for non-focused panes.
pub const BORDER_DIP: f32 = 1.0;
/// Border thickness for the focused pane.
pub const BORDER_ACTIVE_DIP: f32 = 2.0;
/// Minimum visible width for one tab in DIPs. Matches the floor used by
/// the slot-width computation.
pub const TAB_MIN_WIDTH_DIP: f32 = 120.0;
/// D5 minimum readable width for one tab in DIPs — ~20 chars at
/// [`STRIP_FONT_SIZE_DIP`]. The shrink-then-wrap layout
/// (`tab_strip_layout`) refuses to scale below this; once every tab is
/// at this width and the strip would still overflow, it wraps onto
/// additional rows.
pub const TAB_MIN_READABLE_WIDTH_DIP: f32 = 120.0;
const TAB_TOP_GAP_DIP: f32 = 5.0;

/// Compute the width allocated to each tab in a strip of total `strip_w`
/// DIPs. Used both by the paint code and by the UI hit-test so a click on
/// the rendered tab N actually picks tab N.
///
/// The algorithm matches `paint_one_pane_strip` exactly: each tab gets an
/// approximate text-width estimate clamped to [`TAB_MIN_WIDTH_DIP`]; if
/// the sum exceeds `strip_w`, every slot is scaled down proportionally.
#[must_use]
pub fn tab_slot_widths(labels: &[&str], strip_w: f32) -> Vec<f32> {
    if labels.is_empty() || strip_w <= 0.0 {
        return Vec::new();
    }
    let mut widths: Vec<f32> = Vec::with_capacity(labels.len());
    let mut total = 0.0;
    for label in labels {
        let chars = label.chars().count() as f32;
        let est = chars * STRIP_FONT_SIZE_DIP * 0.55 + TAB_PADDING_DIP * 2.0;
        let w = est.max(TAB_MIN_WIDTH_DIP);
        widths.push(w);
        total += w;
    }
    let scale = if total > strip_w {
        strip_w / total
    } else {
        1.0
    };
    let mut cursor = 0.0;
    for w in widths.iter_mut() {
        let allotted = (*w * scale).min((strip_w - cursor).max(0.0));
        *w = allotted;
        cursor += allotted;
    }
    widths
}

/// Map a click x-offset (relative to the strip's left edge) to a tab index
/// using the same widths as [`tab_slot_widths`]. Returns `None` if the
/// click is past the last tab.
#[must_use]
pub fn tab_index_at(widths: &[f32], x_offset: f32) -> Option<usize> {
    if widths.is_empty() || x_offset < 0.0 {
        return None;
    }
    let mut acc = 0.0;
    for (i, w) in widths.iter().enumerate() {
        acc += *w;
        if x_offset < acc {
            return Some(i);
        }
    }
    None
}

/// One row of a multi-row tab strip — slot widths summing to `strip_w`.
/// `slot_tabs` is the slice indices into the original `labels` array
/// that landed in this row (in positional order).
#[derive(Debug, Clone, PartialEq)]
pub struct TabStripRow {
    /// Tab indices (positional, into the original `labels` array) that
    /// occupy this row.
    pub tab_indices: Vec<usize>,
    /// Per-slot widths for this row.
    pub widths: Vec<f32>,
}

/// Full layout for a D5 shrink-then-wrap tab strip.
#[derive(Debug, Clone, PartialEq)]
pub struct TabStripLayout {
    /// Rows in painted-top-to-bottom order. Always contains at least one
    /// row when `labels` is non-empty.
    pub rows: Vec<TabStripRow>,
}

/// D5 — compute a shrink-then-wrap layout for a tab strip.
///
/// Algorithm:
/// 1. Estimate each tab's preferred width via the same per-character
///    metric used by [`tab_slot_widths`].
/// 2. If the sum fits in `strip_w` at preferred widths → one row, no
///    scaling (preserves nice trailing whitespace at the right edge).
/// 3. Otherwise greedily pack tabs into rows: each tab claims
///    `max(preferred, TAB_MIN_READABLE_WIDTH_DIP)`. When adding the
///    next tab would push the row past `strip_w`, close the row and
///    start a new one. Within a row, if the sum of preferred widths
///    exceeds `strip_w`, every slot scales down proportionally — but
///    never below [`TAB_MIN_READABLE_WIDTH_DIP`].
/// 4. Empty `labels` or non-positive `strip_w` → empty layout.
///
/// `strip_w` smaller than `TAB_MIN_READABLE_WIDTH_DIP` is the
/// degenerate case: every tab gets its own row at width `strip_w`
/// (the user has made the pane too narrow for even one minimum tab).
#[must_use]
pub fn tab_strip_layout(labels: &[&str], strip_w: f32) -> TabStripLayout {
    if labels.is_empty() || strip_w <= 0.0 {
        return TabStripLayout { rows: Vec::new() };
    }
    // Preferred width per tab (same metric as tab_slot_widths).
    let preferred: Vec<f32> = labels
        .iter()
        .map(|label| {
            let chars = label.chars().count() as f32;
            let est = chars * STRIP_FONT_SIZE_DIP * 0.55 + TAB_PADDING_DIP * 2.0;
            est.max(TAB_MIN_READABLE_WIDTH_DIP)
        })
        .collect();
    let total: f32 = preferred.iter().sum();
    // Single-row fast path: everything fits unscaled.
    if total <= strip_w {
        return TabStripLayout {
            rows: vec![TabStripRow {
                tab_indices: (0..labels.len()).collect(),
                widths: preferred,
            }],
        };
    }
    // Multi-row: pack greedily.
    let mut rows: Vec<TabStripRow> = Vec::new();
    let mut row_indices: Vec<usize> = Vec::new();
    let mut row_widths: Vec<f32> = Vec::new();
    let mut row_total: f32 = 0.0;
    for (i, w) in preferred.iter().enumerate() {
        let next_total = row_total + *w;
        let would_overflow = next_total > strip_w && !row_indices.is_empty();
        if would_overflow {
            rows.push(TabStripRow {
                tab_indices: std::mem::take(&mut row_indices),
                widths: std::mem::take(&mut row_widths),
            });
            row_total = 0.0;
        }
        row_indices.push(i);
        row_widths.push(*w);
        row_total += *w;
    }
    if !row_indices.is_empty() {
        rows.push(TabStripRow {
            tab_indices: row_indices,
            widths: row_widths,
        });
    }
    // Within each row, if the row total exceeds the strip, scale down —
    // but never below TAB_MIN_READABLE_WIDTH_DIP. (Only matters when a
    // single tab's preferred width exceeds strip_w, in which case that
    // tab sits alone in its row.)
    for row in rows.iter_mut() {
        let row_total: f32 = row.widths.iter().sum();
        if row_total > strip_w {
            let scale = strip_w / row_total;
            for w in row.widths.iter_mut() {
                *w = (*w * scale).max(TAB_MIN_READABLE_WIDTH_DIP.min(strip_w));
            }
        }
    }
    TabStripLayout { rows }
}

/// Phase H2 dispatcher — skips when DF hides both strip and borders
/// and no tab-drag affordance needs to paint.
#[allow(clippy::missing_errors_doc)]
pub fn dispatch_pane_chrome(
    d2d: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    params: &crate::params::DrawParams<'_>,
) -> Result<(), Error> {
    let strip = params.view_options.show_tab_strip;
    let borders = params.view_options.show_pane_borders;
    let Some(c) = params.pane_chrome else {
        return Ok(());
    };
    let has_drag = c.tab_drag.is_some();
    if !(strip || borders || has_drag) {
        return Ok(());
    }
    if strip || borders {
        paint_pane_chrome(d2d, dwrite, c, "Segoe UI", "en-us", strip, borders)?;
    }
    // In-flight tab drag affordance paints over the strip / pane
    // borders so the insertion bar and pane-body highlight land on
    // top of the chrome the user already sees.
    crate::tab_drag_paint::paint_tab_drag_overlay(d2d, dwrite, c, "Segoe UI", "en-us")
}

/// Paint every pane's tab strip + border. Caller has already issued
/// `BeginDraw`; this function does NOT call `EndDraw` / `Present`.
///
/// Phase H2: `show_tab_strip` / `show_pane_borders` independently gate
/// the two paint passes. Distraction-free mode drops both `false`.
pub fn paint_pane_chrome(
    d2d: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    chrome: &PaneChromeDraw,
    font_family: &str,
    locale: &str,
    show_tab_strip: bool,
    show_pane_borders: bool,
) -> Result<(), Error> {
    let strip_format = make_strip_format(dwrite, font_family, locale)?;
    unsafe {
        let render_target: windows::Win32::Graphics::Direct2D::ID2D1RenderTarget = d2d.cast()?;
        let factory = render_target.GetFactory()?;
        let bg: D2D1_COLOR_F = chrome.colors.bg.into();
        let fg: D2D1_COLOR_F = chrome.colors.fg.into();
        let active_bg: D2D1_COLOR_F = chrome.colors.active_tab_bg.into();
        let active_fg: D2D1_COLOR_F = chrome.colors.active_tab_fg.into();
        let inactive_bg: D2D1_COLOR_F = chrome.colors.inactive_tab_bg.into();
        let inactive_fg: D2D1_COLOR_F = chrome.colors.inactive_tab_fg.into();
        let border: D2D1_COLOR_F = chrome.colors.pane_border.into();
        let border_active: D2D1_COLOR_F = chrome.colors.pane_border_active.into();
        let bg_brush: ID2D1SolidColorBrush = render_target.CreateSolidColorBrush(&bg, None)?;
        let fg_brush: ID2D1SolidColorBrush = render_target.CreateSolidColorBrush(&fg, None)?;
        let active_bg_brush: ID2D1SolidColorBrush =
            render_target.CreateSolidColorBrush(&active_bg, None)?;
        let active_fg_brush: ID2D1SolidColorBrush =
            render_target.CreateSolidColorBrush(&active_fg, None)?;
        let inactive_bg_brush: ID2D1SolidColorBrush =
            render_target.CreateSolidColorBrush(&inactive_bg, None)?;
        let inactive_fg_brush: ID2D1SolidColorBrush =
            render_target.CreateSolidColorBrush(&inactive_fg, None)?;
        let border_brush: ID2D1SolidColorBrush =
            render_target.CreateSolidColorBrush(&border, None)?;
        let border_active_brush: ID2D1SolidColorBrush =
            render_target.CreateSolidColorBrush(&border_active, None)?;

        for pane in &chrome.panes {
            if show_tab_strip {
                paint_one_pane_strip(
                    d2d,
                    &factory,
                    dwrite,
                    pane,
                    chrome.strip_height,
                    &bg_brush,
                    &fg_brush,
                    &active_bg_brush,
                    &active_fg_brush,
                    &inactive_bg_brush,
                    &inactive_fg_brush,
                    &strip_format,
                )?;
            }
            if show_pane_borders {
                let active_alpha = match (pane.focused, pane.focus_motion) {
                    (_, Some(motion)) => motion.opacity.clamp(0.0, 1.0),
                    (true, None) => 1.0,
                    (false, None) => 0.0,
                };
                if active_alpha <= 0.0 {
                    paint_pane_border(d2d, pane, &border_brush, BORDER_DIP);
                } else if active_alpha >= 1.0 {
                    paint_pane_border(d2d, pane, &border_active_brush, BORDER_ACTIVE_DIP);
                } else {
                    paint_pane_border(d2d, pane, &border_brush, BORDER_DIP);
                    let active_color = with_alpha(chrome.colors.pane_border_active, active_alpha);
                    let active: D2D1_COLOR_F = active_color.into();
                    let active_brush = render_target.CreateSolidColorBrush(&active, None)?;
                    let width = BORDER_DIP + (BORDER_ACTIVE_DIP - BORDER_DIP) * active_alpha;
                    paint_pane_border(d2d, pane, &active_brush, width);
                }
            }
        }
    }
    Ok(())
}

fn make_strip_format(
    dwrite: &IDWriteFactory,
    font_family: &str,
    locale: &str,
) -> Result<IDWriteTextFormat, Error> {
    let family = HSTRING::from(font_family);
    let loc = HSTRING::from(locale);
    let format: IDWriteTextFormat = unsafe {
        dwrite.CreateTextFormat(
            &family,
            None,
            DWRITE_FONT_WEIGHT_REGULAR,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            STRIP_FONT_SIZE_DIP,
            &loc,
        )?
    };
    unsafe {
        format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
        format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
    }
    Ok(format)
}

#[allow(clippy::too_many_arguments)]
fn paint_one_pane_strip(
    d2d: &ID2D1DeviceContext,
    factory: &ID2D1Factory,
    dwrite: &IDWriteFactory,
    pane: &PaneStripDraw,
    strip_h: f32,
    bg_brush: &ID2D1SolidColorBrush,
    fg_brush: &ID2D1SolidColorBrush,
    active_bg_brush: &ID2D1SolidColorBrush,
    active_fg_brush: &ID2D1SolidColorBrush,
    inactive_bg_brush: &ID2D1SolidColorBrush,
    _inactive_fg_brush: &ID2D1SolidColorBrush,
    format: &IDWriteTextFormat,
) -> Result<(), Error> {
    let (x, y, w, h) = pane.outer;
    if w <= 0.0 || h <= 0.0 {
        return Ok(());
    }
    let strip_h = strip_h.min(h);
    let strip_rect = D2D_RECT_F {
        left: x,
        top: y,
        right: x + w,
        bottom: y + strip_h,
    };
    unsafe {
        d2d.FillRectangle(&strip_rect, bg_brush);
    }

    // Each tab gets a slot proportional to its label width with horizontal
    // padding. The width algorithm is in [`tab_slot_widths`] so the UI
    // hit-test can map a click to the same tab the paint code rendered.
    if pane.tabs.is_empty() {
        return Ok(());
    }
    let labels: Vec<&str> = pane.tabs.iter().map(|t| t.text.as_ref()).collect();
    let widths = tab_slot_widths(&labels, w);
    let mut cursor_x = x;
    let tab_top_gap = TAB_TOP_GAP_DIP.min(strip_h * 0.45);
    let tab_y = y + tab_top_gap;
    let tab_h = (strip_h - tab_top_gap).max(1.0);
    for (i, tab) in pane.tabs.iter().enumerate() {
        let tw = widths.get(i).copied().unwrap_or(0.0);
        if tw <= 0.0 {
            break;
        }
        let tab_rect = D2D_RECT_F {
            left: cursor_x,
            top: tab_y,
            right: cursor_x + tw,
            bottom: tab_y + tab_h,
        };
        let is_active = i == pane.active_index;
        let tab_bg_brush = if is_active {
            active_bg_brush
        } else {
            inactive_bg_brush
        };
        paint_tab_background(d2d, factory, tab_rect, tab_bg_brush)?;
        // Tab label. Keep it one-line and clipped to its slot so a
        // long derived title cannot wrap into the strip or close glyph.
        let left_skew_reserve = TAB_TRAPEZOID_SKEW_DIP * 0.5;
        let label_left = cursor_x + TAB_PADDING_DIP + left_skew_reserve;
        let label_w = (tw - TAB_PADDING_DIP * 2.0 - left_skew_reserve).max(0.0);
        if label_w > 1.0 {
            let wide: Vec<u16> = tab.text.encode_utf16().collect();
            let layout_rect = D2D_RECT_F {
                left: label_left,
                top: tab_y,
                right: label_left + label_w,
                bottom: tab_y + tab_h,
            };
            let layout = unsafe {
                dwrite.CreateTextLayout(&wide, format, label_w.max(1.0), tab_h.max(1.0))?
            };
            unsafe {
                layout.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            }
            let brush = if is_active { active_fg_brush } else { fg_brush };
            unsafe {
                d2d.PushAxisAlignedClip(&layout_rect, D2D1_ANTIALIAS_MODE_ALIASED);
                d2d.DrawTextLayout(
                    D2D_POINT_2F {
                        x: layout_rect.left,
                        y: layout_rect.top,
                    },
                    &layout,
                    brush,
                    D2D1_DRAW_TEXT_OPTIONS_CLIP,
                );
                d2d.PopAxisAlignedClip();
            }
        }
        // Optional close button at the right edge of the tab. Painted
        // last so it sits over the label background.
        if tab.show_close && tw >= TAB_CLOSE_MIN_TAB_WIDTH_DIP {
            let close_rect = close_button_rect(cursor_x, tw, tab_y, tab_h);
            let brush = if is_active { active_fg_brush } else { fg_brush };
            paint_close_icon(d2d, close_rect, brush);
        }
        cursor_x += tw;
        if cursor_x >= x + w {
            break;
        }
    }
    // α.2 — active-tab slide underline. Paints a sliding 3 DIP bar
    // from the previous-active tab's rect to the current one over the
    // 160 ms ease-out crossover scheduled by `ChromeMotionState`. The
    // underline is transient: at the end of the span, the active-tab
    // background fill above is the sole remaining indicator.
    crate::pane_chrome_slide::paint_active_tab_slide_underline(
        d2d,
        pane,
        &widths,
        x,
        y,
        strip_h,
        active_fg_brush,
    );
    Ok(())
}

fn with_alpha(color: Rgba, alpha_multiplier: f32) -> Rgba {
    Rgba {
        a: color.a * alpha_multiplier.clamp(0.0, 1.0),
        ..color
    }
}

/// Minimum tab width (DIPs) before the close button is drawn — below
/// this, the label is already cramped and the `×` would overlap it.
pub const TAB_CLOSE_MIN_TAB_WIDTH_DIP: f32 = 60.0;
/// Close-button glyph cell width in DIPs.
pub const TAB_CLOSE_WIDTH_DIP: f32 = 16.0;

#[cfg(test)]
mod strip_layout_tests {
    use super::*;

    #[test]
    fn empty_labels_returns_empty_layout() {
        let l = tab_strip_layout(&[], 800.0);
        assert!(l.rows.is_empty());
    }

    #[test]
    fn non_positive_strip_returns_empty() {
        assert!(tab_strip_layout(&["x"], 0.0).rows.is_empty());
        assert!(tab_strip_layout(&["x"], -10.0).rows.is_empty());
    }

    #[test]
    fn single_row_when_everything_fits() {
        let labels = ["abc", "def"];
        let l = tab_strip_layout(&labels, 800.0);
        assert_eq!(l.rows.len(), 1);
        assert_eq!(l.rows[0].tab_indices, vec![0, 1]);
        assert_eq!(l.rows[0].widths.len(), 2);
        // Each tab gets at least TAB_MIN_READABLE_WIDTH_DIP.
        for w in &l.rows[0].widths {
            assert!(*w >= TAB_MIN_READABLE_WIDTH_DIP - 0.01);
        }
    }

    #[test]
    fn wraps_when_minimum_widths_overflow_strip() {
        // 10 tabs at MIN_READABLE = 120 → 1200 DIP total. Strip = 500.
        // Expect rows of 4 (480 = 4 * 120; next would be 600) → 4 / 4 / 2.
        let labels: Vec<&str> = vec!["t"; 10];
        let l = tab_strip_layout(&labels, 500.0);
        // First row: as many tabs as fit at 120 each before the next
        // overflows. 4 tabs = 480 DIP, adding a 5th = 600 > 500 → wrap.
        assert!(l.rows.len() >= 3);
        for row in &l.rows {
            let total: f32 = row.widths.iter().sum();
            assert!(total <= 500.0 + 0.01, "row total {} exceeds strip", total);
            for w in &row.widths {
                assert!(*w >= TAB_MIN_READABLE_WIDTH_DIP - 0.01);
            }
        }
        // Indices cover every label in order.
        let mut all: Vec<usize> = Vec::new();
        for r in &l.rows {
            all.extend(&r.tab_indices);
        }
        assert_eq!(all, (0..10).collect::<Vec<_>>());
    }

    #[test]
    fn each_row_remains_in_positional_order() {
        let labels: Vec<&str> = vec!["t"; 6];
        let l = tab_strip_layout(&labels, 300.0);
        let mut prev = -1i32;
        for r in &l.rows {
            for i in &r.tab_indices {
                let cur = *i as i32;
                assert!(cur > prev, "positional order broken");
                prev = cur;
            }
        }
    }

    #[test]
    fn long_label_alone_in_row_when_exceeds_strip() {
        // Strip narrower than even one min-readable tab → degenerate
        // case still emits a row per tab so nothing disappears.
        let l = tab_strip_layout(&["a", "b"], 100.0);
        assert_eq!(l.rows.len(), 2);
        for r in &l.rows {
            assert_eq!(r.tab_indices.len(), 1);
        }
    }

    #[test]
    fn preferred_width_grows_with_label_length() {
        // Long label gets a wider preferred slot, but still at least
        // TAB_MIN_READABLE_WIDTH_DIP.
        let long_label = "a".repeat(30);
        let labels = [long_label.as_str()];
        let l = tab_strip_layout(&labels, 800.0);
        let w = l.rows[0].widths[0];
        assert!(w >= TAB_MIN_READABLE_WIDTH_DIP);
    }
}
