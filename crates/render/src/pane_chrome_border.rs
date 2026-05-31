//! Sibling to [`crate::pane_chrome`]: just the per-pane border-rect
//! painter, split out so `pane_chrome.rs` stays under the conventions
//! file-length cap.
//!
//! Thread ownership: caller is the UI thread (sole owner of the
//! `ID2D1DeviceContext`).

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{ID2D1DeviceContext, ID2D1SolidColorBrush};

use crate::params::PaneStripDraw;

/// Paint the four edges of `pane.outer` with `brush` at `thickness`. No-op
/// for zero-area panes or non-positive thicknesses.
pub(crate) fn paint_pane_border(
    d2d: &ID2D1DeviceContext,
    pane: &PaneStripDraw,
    brush: &ID2D1SolidColorBrush,
    thickness: f32,
) {
    let (x, y, w, h) = pane.outer;
    if w <= 0.0 || h <= 0.0 || thickness <= 0.0 {
        return;
    }
    // Bottom edge of tab strip / top edge of body separator.
    let edges = [
        // Top edge.
        D2D_RECT_F {
            left: x,
            top: y,
            right: x + w,
            bottom: y + thickness,
        },
        // Bottom edge.
        D2D_RECT_F {
            left: x,
            top: y + h - thickness,
            right: x + w,
            bottom: y + h,
        },
        // Left edge.
        D2D_RECT_F {
            left: x,
            top: y,
            right: x + thickness,
            bottom: y + h,
        },
        // Right edge.
        D2D_RECT_F {
            left: x + w - thickness,
            top: y,
            right: x + w,
            bottom: y + h,
        },
    ];
    unsafe {
        for r in &edges {
            d2d.FillRectangle(r, brush);
        }
    }
}
