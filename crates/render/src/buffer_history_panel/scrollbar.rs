//! Scrollbar geometry and paint for the buffer-history lane list.

use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_ALIASED,
};

use super::paint_helpers::panel_rect_to_d2d;
use super::{BufferHistoryPanelLayout, BufferHistoryScrollbarLayout, PanelRect, PANEL_PAD_DIP};

/// Width reserved at the panel's right edge when the lane list overflows.
pub(super) const SCROLLBAR_GUTTER_DIP: f32 = 12.0;
const TRACK_WIDTH_DIP: f32 = 4.0;
const TRACK_VERTICAL_PAD_DIP: f32 = 6.0;
const MIN_THUMB_HEIGHT_DIP: f32 = 24.0;

/// Compute a proportional lane-list scrollbar when not every lane fits.
#[must_use]
pub(super) fn compute_scrollbar_layout(
    panel_rect: PanelRect,
    lanes_origin_y: f32,
    lanes_bottom: f32,
    total_lanes: usize,
    visible_lane_capacity: usize,
    scroll_lane_offset: usize,
) -> Option<BufferHistoryScrollbarLayout> {
    if total_lanes <= visible_lane_capacity || visible_lane_capacity == 0 {
        return None;
    }
    let track_h = (lanes_bottom - lanes_origin_y - 2.0 * TRACK_VERTICAL_PAD_DIP).max(0.0);
    if track_h <= 0.0 {
        return None;
    }
    let track_rect = PanelRect {
        x: panel_rect.x + panel_rect.w - PANEL_PAD_DIP - TRACK_WIDTH_DIP,
        y: lanes_origin_y + TRACK_VERTICAL_PAD_DIP,
        w: TRACK_WIDTH_DIP,
        h: track_h,
    };
    let visible_fraction = visible_lane_capacity as f32 / total_lanes.max(1) as f32;
    let thumb_h = (track_h * visible_fraction)
        .max(MIN_THUMB_HEIGHT_DIP.min(track_h))
        .min(track_h);
    let max_scroll = total_lanes.saturating_sub(visible_lane_capacity).max(1);
    let scroll = scroll_lane_offset.min(max_scroll) as f32;
    let progress = scroll / max_scroll as f32;
    let thumb_y = track_rect.y + (track_h - thumb_h).max(0.0) * progress;
    Some(BufferHistoryScrollbarLayout {
        track_rect,
        thumb_rect: PanelRect {
            x: track_rect.x,
            y: thumb_y,
            w: track_rect.w,
            h: thumb_h,
        },
    })
}

/// Paint the lane-list scrollbar when the layout requested one.
pub(super) fn paint_scrollbar(
    ctx: &ID2D1DeviceContext,
    layout: &BufferHistoryPanelLayout,
    track_brush: &ID2D1SolidColorBrush,
    thumb_brush: &ID2D1SolidColorBrush,
) {
    let Some(scrollbar) = layout.scrollbar else {
        return;
    };
    unsafe {
        let clip = panel_rect_to_d2d(layout.background_rect);
        ctx.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED);
        ctx.FillRectangle(&panel_rect_to_d2d(scrollbar.track_rect), track_brush);
        ctx.FillRectangle(&panel_rect_to_d2d(scrollbar.thumb_rect), thumb_brush);
        ctx.PopAxisAlignedClip();
    }
}
