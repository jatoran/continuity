//! Caret-rect geometry, split out of [`crate::chrome`] (Phase B4).
//!
//! Computing the caret's hit-test rectangle is a pure-D2D affair and was
//! contributing to chrome.rs's over-the-cap line count; pulling it into a
//! sibling keeps both files tidy and lets B4's `caret_width_px` plumbing
//! land without further chrome.rs growth.

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;

use crate::params::CaretShape;

/// Compute the caret rect for a single selection. `caret_x` is the
/// leading-edge X from `IDWriteTextLayout::HitTestTextPosition`.
/// `bar_width_px == 0` falls back to the legacy 1.5 DIP bar width
/// (B4: `editor.caret_width_px`).
#[must_use]
pub fn caret_rect_for_shape(
    caret_x: f32,
    line_y: f32,
    line_height: f32,
    column_advance: f32,
    shape: CaretShape,
    bar_width_px: u32,
) -> D2D_RECT_F {
    match shape {
        CaretShape::Bar => {
            let w = if bar_width_px == 0 {
                1.5
            } else {
                bar_width_px as f32
            };
            D2D_RECT_F {
                left: caret_x,
                top: line_y,
                right: caret_x + w,
                bottom: line_y + line_height,
            }
        }
        CaretShape::Block => D2D_RECT_F {
            left: caret_x,
            top: line_y,
            right: caret_x + column_advance.max(2.0),
            bottom: line_y + line_height,
        },
        CaretShape::Underline => D2D_RECT_F {
            left: caret_x,
            top: line_y + line_height - 2.0,
            right: caret_x + column_advance.max(2.0),
            bottom: line_y + line_height,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::caret_rect_for_shape;
    use crate::params::CaretShape;

    #[test]
    fn bar_is_thin_by_default() {
        let r = caret_rect_for_shape(10.0, 0.0, 20.0, 8.0, CaretShape::Bar, 0);
        assert!((r.right - r.left) < 2.0);
    }

    #[test]
    fn bar_honours_configured_width() {
        let r = caret_rect_for_shape(10.0, 0.0, 20.0, 8.0, CaretShape::Bar, 4);
        assert_eq!(r.right - r.left, 4.0);
    }

    #[test]
    fn block_uses_column_advance() {
        let r = caret_rect_for_shape(10.0, 0.0, 20.0, 8.0, CaretShape::Block, 0);
        assert!((r.right - r.left) >= 8.0);
        assert_eq!(r.bottom - r.top, 20.0);
    }

    #[test]
    fn underline_thin_at_bottom() {
        let r = caret_rect_for_shape(10.0, 0.0, 20.0, 8.0, CaretShape::Underline, 0);
        assert!(r.top >= 18.0);
        assert_eq!(r.bottom, 20.0);
    }

    #[test]
    fn block_ignores_bar_width() {
        let r = caret_rect_for_shape(10.0, 0.0, 20.0, 8.0, CaretShape::Block, 99);
        assert!((r.right - r.left) >= 8.0);
        assert!((r.right - r.left) < 99.0);
    }
}
