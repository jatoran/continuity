//! Small tab-strip drawing primitives split from [`crate::pane_chrome`].

use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED, D2D_POINT_2F, D2D_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{
    ID2D1Brush, ID2D1DeviceContext, ID2D1Factory, ID2D1SolidColorBrush, D2D1_ELLIPSE,
};

use crate::pane_chrome::TAB_CLOSE_WIDTH_DIP;
use crate::Error;

pub(crate) const TAB_TRAPEZOID_SKEW_DIP: f32 = 7.0;
pub(crate) const TAB_CLOSE_RIGHT_INSET_DIP: f32 = 8.5;
const TAB_CLOSE_ICON_SIZE_DIP: f32 = 6.25;
const TAB_CLOSE_ICON_STROKE_DIP: f32 = 1.2;
const TAB_CLOSE_ICON_RIGHT_PADDING_DIP: f32 = 1.5;
/// Diameter of the unsaved-buffer dirty dot relative to the close-cell
/// width. Sized so the dot reads as a peer of the `×` close glyph it
/// replaces (matching its right padding + vertical center) and scales
/// with the cell as the strip grows under text zoom.
const TAB_DIRTY_DOT_DIAMETER_FRACTION: f32 = 0.42;

pub(crate) fn paint_tab_background(
    d2d: &ID2D1DeviceContext,
    factory: &ID2D1Factory,
    rect: D2D_RECT_F,
    brush: &ID2D1SolidColorBrush,
) -> Result<(), Error> {
    let width = (rect.right - rect.left).max(0.0);
    let skew = TAB_TRAPEZOID_SKEW_DIP.min(width * 0.25);
    let geometry = unsafe { factory.CreatePathGeometry()? };
    let sink = unsafe { geometry.Open()? };
    unsafe {
        sink.BeginFigure(
            D2D_POINT_2F {
                x: rect.left + skew,
                y: rect.top,
            },
            D2D1_FIGURE_BEGIN_FILLED,
        );
        sink.AddLines(&[
            D2D_POINT_2F {
                x: rect.right - skew,
                y: rect.top,
            },
            D2D_POINT_2F {
                x: rect.right,
                y: rect.bottom,
            },
            D2D_POINT_2F {
                x: rect.left,
                y: rect.bottom,
            },
        ]);
        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        sink.Close()?;
        d2d.FillGeometry(&geometry, brush, Option::<&ID2D1Brush>::None);
    }
    Ok(())
}

/// Rect of the close-icon cell for a tab whose strip-relative origin is
/// `(tab_x, tab_y)` with width `tab_w` and strip height `strip_h`.
#[must_use]
pub fn close_button_rect(tab_x: f32, tab_w: f32, tab_y: f32, strip_h: f32) -> D2D_RECT_F {
    let right = tab_x + tab_w - TAB_CLOSE_RIGHT_INSET_DIP;
    let left = right - TAB_CLOSE_WIDTH_DIP;
    D2D_RECT_F {
        left,
        top: tab_y,
        right,
        bottom: tab_y + strip_h,
    }
}

pub(crate) fn paint_close_icon(
    d2d: &ID2D1DeviceContext,
    rect: D2D_RECT_F,
    brush: &ID2D1SolidColorBrush,
) {
    let icon_size = TAB_CLOSE_ICON_SIZE_DIP
        .min((rect.right - rect.left - 4.0).max(1.0))
        .min((rect.bottom - rect.top - 4.0).max(1.0));
    let half = icon_size * 0.5;
    let center_x = rect.right - TAB_CLOSE_ICON_RIGHT_PADDING_DIP - half;
    let center_y = (rect.top + rect.bottom) * 0.5;
    unsafe {
        d2d.PushAxisAlignedClip(
            &rect,
            windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_ALIASED,
        );
        d2d.DrawLine(
            D2D_POINT_2F {
                x: center_x - half,
                y: center_y - half,
            },
            D2D_POINT_2F {
                x: center_x + half,
                y: center_y + half,
            },
            brush,
            TAB_CLOSE_ICON_STROKE_DIP,
            None,
        );
        d2d.DrawLine(
            D2D_POINT_2F {
                x: center_x + half,
                y: center_y - half,
            },
            D2D_POINT_2F {
                x: center_x - half,
                y: center_y + half,
            },
            brush,
            TAB_CLOSE_ICON_STROKE_DIP,
            None,
        );
        d2d.PopAxisAlignedClip();
    }
}

/// Paint a filled dirty-state dot inside the close-button cell `rect`.
///
/// Drawn only when the tab is *not* hovered: on hover the close `×`
/// takes the same cell (see `paint_one_pane_strip`), so the dot and the
/// `×` are mutually exclusive and occupy the identical position. The
/// dot's center matches [`paint_close_icon`]'s glyph center (same right
/// padding, same vertical midline) so toggling hover does not shift the
/// affordance. Diameter scales with the cell via
/// [`TAB_DIRTY_DOT_DIAMETER_FRACTION`] so it grows with text zoom.
pub(crate) fn paint_dirty_dot(
    d2d: &ID2D1DeviceContext,
    rect: D2D_RECT_F,
    brush: &ID2D1SolidColorBrush,
) {
    let cell_w = (rect.right - rect.left).max(0.0);
    let cell_h = (rect.bottom - rect.top).max(0.0);
    if cell_w <= 0.0 || cell_h <= 0.0 {
        return;
    }
    let diameter = (cell_w * TAB_DIRTY_DOT_DIAMETER_FRACTION)
        .min(cell_h * TAB_DIRTY_DOT_DIAMETER_FRACTION)
        .max(1.0);
    let radius = diameter * 0.5;
    // Match `paint_close_icon`: the close glyph's center sits a half
    // icon-width left of `rect.right - TAB_CLOSE_ICON_RIGHT_PADDING_DIP`.
    let icon_size = TAB_CLOSE_ICON_SIZE_DIP
        .min((cell_w - 4.0).max(1.0))
        .min((cell_h - 4.0).max(1.0));
    let center_x = rect.right - TAB_CLOSE_ICON_RIGHT_PADDING_DIP - icon_size * 0.5;
    let center_y = (rect.top + rect.bottom) * 0.5;
    let ellipse = D2D1_ELLIPSE {
        point: D2D_POINT_2F {
            x: center_x,
            y: center_y,
        },
        radiusX: radius,
        radiusY: radius,
    };
    unsafe {
        d2d.FillEllipse(&ellipse, brush);
    }
}
