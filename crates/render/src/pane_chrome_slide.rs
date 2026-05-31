//! Sibling to [`crate::pane_chrome`]: active-tab slide underline.
//!
//! On tab activation, the focused tab's indicator slides along the
//! strip's x-axis from the previously-active tab's slot to the new
//! active tab's slot. The 160 ms / ease-out-cubic timing lives in
//! `crates/ui/src/chrome_motion.rs`; this module only consumes the
//! per-frame `(progress, previous_active_tab_index)` projection on
//! `PaneStripDraw` and paints a 3 DIP underline at the lerped rect.
//!
//! Thread ownership: caller is the UI thread (sole owner of the
//! `ID2D1DeviceContext`).

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{ID2D1DeviceContext, ID2D1SolidColorBrush};

use crate::params::PaneStripDraw;

/// Active-tab slide underline height in DIPs.
pub(crate) const TAB_UNDERLINE_DIP: f32 = 3.0;

/// Paint the active-tab slide underline for `pane`. No-op when the slide
/// transient projection is absent (`active_tab_motion` or
/// `previous_active_tab_index` is `None`) or when either tab rect cannot
/// be computed (e.g. index past the visible slot set).
pub(crate) fn paint_active_tab_slide_underline(
    d2d: &ID2D1DeviceContext,
    pane: &PaneStripDraw,
    widths: &[f32],
    x: f32,
    y: f32,
    strip_h: f32,
    active_fg_brush: &ID2D1SolidColorBrush,
) {
    let (Some(motion), Some(prev_idx)) = (pane.active_tab_motion, pane.previous_active_tab_index)
    else {
        return;
    };
    let Some(prev_rect) = tab_underline_rect(widths, prev_idx, x, y, strip_h) else {
        return;
    };
    let Some(cur_rect) = tab_underline_rect(widths, pane.active_index, x, y, strip_h) else {
        return;
    };
    let progress = motion.opacity.clamp(0.0, 1.0);
    let lerp = |a: f32, b: f32| a + (b - a) * progress;
    let underline = D2D_RECT_F {
        left: lerp(prev_rect.left, cur_rect.left),
        top: lerp(prev_rect.top, cur_rect.top),
        right: lerp(prev_rect.right, cur_rect.right),
        bottom: lerp(prev_rect.bottom, cur_rect.bottom),
    };
    unsafe {
        d2d.FillRectangle(&underline, active_fg_brush);
    }
}

/// Slide-underline rect for the tab at `index` inside a strip whose
/// per-slot widths are `widths` and whose top-left is `(x, y)` with
/// height `strip_h`. The rect is a [`TAB_UNDERLINE_DIP`]-tall bar pinned
/// to the bottom of the strip. Returns `None` when `index` is past the
/// visible slot set or when the slot has zero width.
#[must_use]
pub(crate) fn tab_underline_rect(
    widths: &[f32],
    index: usize,
    x: f32,
    y: f32,
    strip_h: f32,
) -> Option<D2D_RECT_F> {
    let mut cursor = x;
    for (i, w) in widths.iter().enumerate() {
        if *w <= 0.0 {
            return None;
        }
        if i == index {
            return Some(D2D_RECT_F {
                left: cursor,
                top: y + strip_h - TAB_UNDERLINE_DIP,
                right: cursor + *w,
                bottom: y + strip_h,
            });
        }
        cursor += *w;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn underline_rect_pins_to_strip_bottom() {
        let widths = vec![120.0, 100.0, 80.0];
        let rect = tab_underline_rect(&widths, 1, 0.0, 0.0, 24.0).expect("present");
        assert!((rect.left - 120.0).abs() < 1e-3);
        assert!((rect.right - 220.0).abs() < 1e-3);
        assert!((rect.top - (24.0 - TAB_UNDERLINE_DIP)).abs() < 1e-3);
        assert!((rect.bottom - 24.0).abs() < 1e-3);
    }

    #[test]
    fn underline_rect_missing_index_returns_none() {
        let widths = vec![120.0, 100.0];
        assert!(tab_underline_rect(&widths, 5, 0.0, 0.0, 24.0).is_none());
    }

    #[test]
    fn underline_rect_zero_width_slot_returns_none() {
        // Once a slot collapses to zero width, every later slot is off
        // screen — the underline cannot pick up a position for them.
        let widths = vec![0.0, 100.0];
        assert!(tab_underline_rect(&widths, 1, 0.0, 0.0, 24.0).is_none());
    }
}
