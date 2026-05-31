//! Minimal vertical-scrollbar painter for the focused pane body.
//!
//! Paints a single thin thumb at the right edge of the editor-body
//! column when the rope's total content height exceeds the viewport
//! height. No track fill, no buttons. Painted between the pane-chrome
//! pass and the modal-overlay pass in
//! [`crate::renderer_post_body::paint_post_body`] so overlays occlude
//! it but tab-strip / pane-border chrome don't.
//!
//! The thumb/track geometry math is also exposed via
//! [`compute_scrollbar_layout`] / [`scroll_y_for_thumb_top`] so the UI
//! layer can hit-test the thumb (cursor change on hover, click-drag
//! the thumb, click the track to page-jump) using the same numbers
//! the painter uses. Keeping a single source of geometry truth means
//! the visual and the hit target can't drift apart.
//!
//! **Thread ownership**: UI thread.

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{ID2D1DeviceContext, ID2D1SolidColorBrush};

use crate::display_projection::FrameDisplay;

/// Width of the thumb in DIPs.
pub const THUMB_WIDTH_DIP: f32 = 4.0;
/// Gap between the text-column right edge and the thumb's left edge.
/// Keeps the thumb in the right-margin gutter so it never paints on
/// top of body text reaching the column edge.
pub const RIGHT_INSET_DIP: f32 = 2.0;
/// Minimum thumb height so a very short proportion still reads as a
/// distinct affordance and provides a usable drag target.
pub const MIN_THUMB_HEIGHT_DIP: f32 = 24.0;
/// Horizontal slop added to the thumb's left edge for hit-testing so
/// the affordance is easier to grab than its 4 DIP visual width.
pub const HIT_LEFT_SLOP_DIP: f32 = 6.0;

/// Geometry needed to paint and hit-test the scrollbar for one pane.
///
/// All coordinates are in client-area DIPs (identity transform).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollbarLayout {
    /// Left edge of the visible thumb.
    pub thumb_left: f32,
    /// Top edge of the visible thumb.
    pub thumb_top: f32,
    /// Right edge of the visible thumb (also the track right edge).
    pub thumb_right: f32,
    /// Bottom edge of the visible thumb.
    pub thumb_bottom: f32,
    /// Top of the track (same as `body_origin_y_dip`).
    pub track_top: f32,
    /// Bottom of the track (`track_top + viewport_h`).
    pub track_bottom: f32,
    /// Total content height (used by hit-test → scroll conversions).
    pub content_h: f32,
    /// Visible viewport height (== track height).
    pub viewport_h: f32,
}

impl ScrollbarLayout {
    /// Track height in DIPs (always equal to `viewport_h`).
    #[inline]
    pub fn track_h(&self) -> f32 {
        self.viewport_h
    }

    /// Thumb height in DIPs.
    #[inline]
    pub fn thumb_h(&self) -> f32 {
        self.thumb_bottom - self.thumb_top
    }

    /// Maximum scroll offset (`content_h - viewport_h`, never negative).
    #[inline]
    pub fn scroll_max(&self) -> f32 {
        (self.content_h - self.viewport_h).max(0.0)
    }

    /// True when `(x, y)` lies within the visible thumb rect (expanded
    /// by [`HIT_LEFT_SLOP_DIP`] on the left so the 4 DIP visual is
    /// easier to grab).
    pub fn hit_test_thumb(&self, x: f32, y: f32) -> bool {
        x >= self.thumb_left - HIT_LEFT_SLOP_DIP
            && x <= self.thumb_right
            && y >= self.thumb_top
            && y <= self.thumb_bottom
    }

    /// True when `(x, y)` is on the scrollbar track but outside the
    /// thumb. Used for page-up / page-down click behaviour.
    pub fn hit_test_track_outside_thumb(&self, x: f32, y: f32) -> bool {
        let on_track = x >= self.thumb_left - HIT_LEFT_SLOP_DIP
            && x <= self.thumb_right
            && y >= self.track_top
            && y <= self.track_bottom;
        on_track && !self.hit_test_thumb(x, y)
    }
}

/// Content height used by scrollbar paint and hit testing.
///
/// The scrollbar tracks display rows, not source lines. Soft-wrap,
/// folds, reveal markers, and image reservations all affect the row
/// count, so scrollbar paint must use the same row index the text
/// projection uses.
pub(crate) fn content_height_for_scrollbar(
    frame_display: &FrameDisplay,
    line_height_dip: f32,
) -> f32 {
    frame_display.display_line_count().max(1) as f32 * line_height_dip
}

/// Compute the scrollbar geometry for a pane, or `None` when nothing
/// overflows the viewport.
///
/// `right_edge_x_dip` is the rightmost x of the text-body column
/// (i.e. `body_origin.x + margins.left + editor_w`); the thumb sits
/// in the right-margin gutter just past that edge, offset by
/// [`RIGHT_INSET_DIP`] so it never paints on top of body text that
/// reaches the column's right edge.
pub fn compute_scrollbar_layout(
    right_edge_x_dip: f32,
    body_origin_y_dip: f32,
    scroll_y_dip: f32,
    viewport_h: f32,
    content_h: f32,
) -> Option<ScrollbarLayout> {
    if viewport_h <= 0.0 || content_h <= 0.0 || content_h <= viewport_h {
        return None;
    }
    let track_top = body_origin_y_dip;
    let track_h = viewport_h;
    let visible_fraction = (viewport_h / content_h).clamp(0.0, 1.0);
    let thumb_h = (track_h * visible_fraction)
        .max(MIN_THUMB_HEIGHT_DIP)
        .min(track_h);
    let scroll_max = (content_h - viewport_h).max(1.0);
    let travel = (track_h - thumb_h).max(0.0);
    let thumb_offset = (scroll_y_dip / scroll_max).clamp(0.0, 1.0) * travel;
    let thumb_top = track_top + thumb_offset;
    let thumb_left = right_edge_x_dip + RIGHT_INSET_DIP;
    let thumb_right = thumb_left + THUMB_WIDTH_DIP;
    Some(ScrollbarLayout {
        thumb_left,
        thumb_top,
        thumb_right,
        thumb_bottom: thumb_top + thumb_h,
        track_top,
        track_bottom: track_top + track_h,
        content_h,
        viewport_h,
    })
}

/// Convert a desired new top-of-thumb y (in client DIPs) into the
/// scroll offset that would place the thumb there. Result is clamped
/// to `[0, scroll_max]`. Used by drag handlers.
pub fn scroll_y_for_thumb_top(layout: &ScrollbarLayout, new_thumb_top_dip: f32) -> f32 {
    let track_h = layout.track_h();
    let thumb_h = layout.thumb_h();
    let travel = (track_h - thumb_h).max(0.0);
    if travel <= 0.0 {
        return 0.0;
    }
    let frac = ((new_thumb_top_dip - layout.track_top) / travel).clamp(0.0, 1.0);
    frac * layout.scroll_max()
}

/// Paint the minimal y-scrollbar thumb at the right edge of the
/// focused pane's editor-body column.
///
/// No-op when `viewport_h >= content_h` (nothing overflows) or when
/// either dimension is non-positive.
///
/// # Safety
///
/// Caller is mid-`BeginDraw`. The current D2D transform must place
/// `right_edge_x_dip` / `body_origin_y_dip` in client-area DIPs
/// (identity transform, matching the post-body paint phase).
pub(crate) unsafe fn paint_scrollbar(
    ctx: &ID2D1DeviceContext,
    right_edge_x_dip: f32,
    body_origin_y_dip: f32,
    scroll_y_dip: f32,
    viewport_h: f32,
    content_h: f32,
    brush: &ID2D1SolidColorBrush,
) {
    let Some(layout) = compute_scrollbar_layout(
        right_edge_x_dip,
        body_origin_y_dip,
        scroll_y_dip,
        viewport_h,
        content_h,
    ) else {
        return;
    };
    let rect = D2D_RECT_F {
        left: layout.thumb_left,
        top: layout.thumb_top,
        right: layout.thumb_right,
        bottom: layout.thumb_bottom,
    };
    ctx.FillRectangle(&rect, brush);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout_for(scroll_y: f32, viewport_h: f32, content_h: f32) -> ScrollbarLayout {
        compute_scrollbar_layout(1000.0, 100.0, scroll_y, viewport_h, content_h)
            .expect("content overflows in this fixture")
    }

    #[test]
    fn thumb_height_uses_visible_fraction() {
        let layout = layout_for(0.0, 200.0, 1000.0);
        assert!((layout.thumb_h() - 40.0).abs() < 0.001);
    }

    #[test]
    fn thumb_clamps_to_minimum_height() {
        let layout = layout_for(0.0, 200.0, 10_000.0);
        assert!((layout.thumb_h() - MIN_THUMB_HEIGHT_DIP).abs() < 0.001);
    }

    #[test]
    fn thumb_offset_reaches_bottom_when_scrolled_to_end() {
        let layout = layout_for(800.0, 200.0, 1000.0);
        assert!((layout.thumb_bottom - layout.track_bottom).abs() < 0.001);
    }

    #[test]
    fn no_layout_when_content_fits() {
        assert!(compute_scrollbar_layout(1000.0, 100.0, 0.0, 200.0, 150.0).is_none());
    }

    #[test]
    fn no_layout_when_viewport_nonpositive() {
        assert!(compute_scrollbar_layout(1000.0, 100.0, 0.0, 0.0, 500.0).is_none());
    }

    #[test]
    fn hit_test_thumb_matches_painted_rect_with_slop() {
        let layout = layout_for(0.0, 200.0, 1000.0);
        let mid_x = (layout.thumb_left + layout.thumb_right) * 0.5;
        let mid_y = (layout.thumb_top + layout.thumb_bottom) * 0.5;
        assert!(layout.hit_test_thumb(mid_x, mid_y));
        assert!(layout.hit_test_thumb(layout.thumb_left - HIT_LEFT_SLOP_DIP + 0.1, mid_y));
        assert!(!layout.hit_test_thumb(layout.thumb_left - HIT_LEFT_SLOP_DIP - 0.1, mid_y));
        assert!(!layout.hit_test_thumb(mid_x, layout.thumb_bottom + 0.1));
    }

    #[test]
    fn track_outside_thumb_excludes_thumb_region() {
        let layout = layout_for(0.0, 200.0, 1000.0);
        let mid_x = (layout.thumb_left + layout.thumb_right) * 0.5;
        let below_thumb_y = layout.thumb_bottom + 5.0;
        assert!(layout.hit_test_track_outside_thumb(mid_x, below_thumb_y));
        let mid_y = (layout.thumb_top + layout.thumb_bottom) * 0.5;
        assert!(!layout.hit_test_track_outside_thumb(mid_x, mid_y));
    }

    #[test]
    fn scroll_y_for_thumb_top_round_trips_endpoints() {
        let layout = layout_for(0.0, 200.0, 1000.0);
        let scroll_at_top = scroll_y_for_thumb_top(&layout, layout.track_top);
        assert!(scroll_at_top.abs() < 0.001);
        let travel = layout.track_h() - layout.thumb_h();
        let scroll_at_bottom = scroll_y_for_thumb_top(&layout, layout.track_top + travel);
        assert!((scroll_at_bottom - layout.scroll_max()).abs() < 0.001);
    }

    #[test]
    fn scroll_y_for_thumb_top_clamps_out_of_range() {
        let layout = layout_for(0.0, 200.0, 1000.0);
        assert!(scroll_y_for_thumb_top(&layout, -1000.0).abs() < 0.001);
        let huge = scroll_y_for_thumb_top(&layout, 1_000_000.0);
        assert!((huge - layout.scroll_max()).abs() < 0.001);
    }

    #[test]
    fn content_height_for_scrollbar_uses_display_row_count() {
        let rope = ropey::Rope::from_str("one two three four five six seven eight nine ten");
        let frame_display = FrameDisplay::build(&rope, 1, None, &[], 10, 1.0);
        let height = content_height_for_scrollbar(&frame_display, 20.0);
        let expected = frame_display.display_line_count().max(1) as f32 * 20.0;
        assert!((height - expected).abs() < 0.001);
    }
}
