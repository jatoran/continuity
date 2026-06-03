//! Pure swimlane layout for the buffer-history panel and the matching
//! hit-test. Sibling of `buffer_history_panel.rs`; the layout is pure
//! (no Direct2D / DirectWrite), so the UI hit-test path consumes it
//! without owning a renderer.

use super::metrics::resolved_metrics;
use super::scrollbar;
use super::{BufferHistoryLaneLayout, BufferHistoryPanelDraw, BufferHistoryPanelLayout, PanelRect};

impl BufferHistoryPanelDraw {
    /// Compute the projected pixel-x for `ts_ms` inside `strip_rect`.
    /// Out-of-range values clamp to the strip's edges; callers paint
    /// only points whose `t in [viewport_start_ms, viewport_end_ms]`.
    #[must_use]
    pub fn project_x(&self, ts_ms: i64, strip_rect: &PanelRect) -> f32 {
        let span = (self.viewport_end_ms - self.viewport_start_ms).max(1) as f64;
        let dx = (ts_ms - self.viewport_start_ms) as f64 / span;
        let dx = dx.clamp(0.0, 1.0);
        strip_rect.x + (dx as f32) * strip_rect.w
    }
}

/// Compute the swimlane layout for `draw`. Pure: no Direct2D, no
/// `IDWriteFactory` — safe to call from a hit-test path that doesn't
/// own the renderer.
#[must_use]
pub fn compute_buffer_history_panel_layout(
    draw: &BufferHistoryPanelDraw,
) -> BufferHistoryPanelLayout {
    let metrics = resolved_metrics(draw.scale);
    let bg = draw.rect;
    let ruler_rect = PanelRect {
        x: bg.x,
        y: bg.y,
        w: bg.w,
        h: metrics.ruler_height.min(bg.h),
    };
    let lanes_origin_y = ruler_rect.y + ruler_rect.h;

    // Reserve the preview band along the bottom when there is at
    // least one lane and the panel can spare the vertical room (need
    // space for the ruler + at least one lane + the band itself).
    let min_required_for_band = ruler_rect.h + metrics.lane_height + metrics.preview_band_height;
    let preview_rect = if !draw.rows.is_empty() && bg.h >= min_required_for_band {
        Some(PanelRect {
            x: bg.x,
            y: bg.y + bg.h - metrics.preview_band_height,
            w: bg.w,
            h: metrics.preview_band_height,
        })
    } else {
        None
    };
    let lanes_bottom = match preview_rect {
        Some(p) => p.y,
        None => bg.y + bg.h,
    };
    let lane_span_h = (lanes_bottom - lanes_origin_y).max(0.0);
    let visible_lane_capacity = (lane_span_h / metrics.lane_height).floor().max(0.0) as usize;
    let scrollbar_layout = scrollbar::compute_scrollbar_layout(
        bg,
        lanes_origin_y,
        lanes_bottom,
        draw.rows.len(),
        visible_lane_capacity,
        draw.scroll_lane_offset,
        &metrics,
    );

    let inner_x = bg.x + metrics.panel_pad;
    let scrollbar_gutter = scrollbar_layout
        .as_ref()
        .map(|_| metrics.scrollbar_gutter)
        .unwrap_or(0.0);
    let inner_w = (bg.w - 2.0 * metrics.panel_pad - scrollbar_gutter).max(0.0);
    let title_w = metrics.title_column_width.min(inner_w * 0.5);
    let strip_x = inner_x + title_w + metrics.panel_pad;
    let strip_w = (inner_w - title_w - metrics.panel_pad).max(0.0);
    let row_w = scrollbar_layout
        .as_ref()
        .map(|s| (s.track_rect.x - bg.x - 4.0 * draw.scale.max(1.0)).max(0.0))
        .unwrap_or(bg.w);

    let scroll = draw.scroll_lane_offset.min(draw.rows.len());
    let mut lanes = Vec::with_capacity(draw.rows.len().saturating_sub(scroll));
    for (visible_idx, (lane_idx, row)) in draw.rows.iter().enumerate().skip(scroll).enumerate() {
        let row_y = lanes_origin_y + (visible_idx as f32) * metrics.lane_height;
        if row_y + metrics.lane_height > lanes_bottom {
            break;
        }
        let row_rect = PanelRect {
            x: bg.x,
            y: row_y,
            w: row_w,
            h: metrics.lane_height,
        };
        // Strip stays vertically centered inside the row so the
        // snapshot dots line up across lanes and read as a continuous
        // horizontal timeline rather than dancing up-and-down with
        // each row's title block.
        let strip_h = metrics.strip_height;
        let strip_rect = PanelRect {
            x: strip_x,
            y: row_y + (metrics.lane_height - strip_h) / 2.0,
            w: strip_w,
            h: strip_h,
        };
        let dot_centers_x = row
            .snapshot_times_ms
            .iter()
            .filter(|&&t| t >= draw.viewport_start_ms && t <= draw.viewport_end_ms)
            .map(|&t| draw.project_x(t, &strip_rect))
            .collect();
        lanes.push(BufferHistoryLaneLayout {
            lane_index: lane_idx,
            row_rect,
            strip_rect,
            dot_centers_x,
        });
    }

    BufferHistoryPanelLayout {
        background_rect: bg,
        ruler_rect,
        lanes,
        preview_rect,
        scrollbar: scrollbar_layout,
        visible_lane_capacity,
    }
}

/// Pointer hit-test against a computed layout. Returns the lane
/// index whose `row_rect` contains `(x, y)`, or `None` when the
/// point lies in the ruler band / between rows / outside the
/// panel.
#[must_use]
pub fn hit_test_lane(layout: &BufferHistoryPanelLayout, x: f32, y: f32) -> Option<usize> {
    for lane in &layout.lanes {
        if rect_contains(&lane.row_rect, x, y) {
            return Some(lane.lane_index);
        }
    }
    None
}

fn rect_contains(rect: &PanelRect, x: f32, y: f32) -> bool {
    x >= rect.x && x <= rect.x + rect.w && y >= rect.y && y <= rect.y + rect.h
}
