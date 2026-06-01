//! Scaled-text minimap mouse interaction for the focused pane.
//!
//! Paint stores the minimap layout on [`crate::window_view_options::ViewOptions`].
//! This module consumes that cached geometry for click-to-scroll and
//! drag-to-scroll so the hit target exactly matches the rendered strip.
//!
//! Thread ownership: all state mutated here is owned by the window UI
//! thread.

use windows::Win32::UI::Input::KeyboardAndMouse::SetCapture;

use crate::window::Window;
use crate::window_helpers::invalidate_hwnd;

/// Map a minimap hit line to an editor scroll target that centers the
/// clicked line in the viewport when there is enough content.
#[must_use]
pub(crate) fn compute_minimap_target_scroll(
    line: u64,
    viewport_height_dip: f32,
    line_height_dip: f32,
    content_height_dip: f32,
) -> f32 {
    let target = line as f32 * line_height_dip - viewport_height_dip * 0.5;
    let max_scroll = (content_height_dip - viewport_height_dip.max(0.0)).max(0.0);
    target.clamp(0.0, max_scroll)
}

impl Window {
    /// `WM_LBUTTONDOWN` hit-test against the scaled-text minimap. On a
    /// hit, scroll the editor so the clicked line is roughly centered
    /// and capture the mouse for continuous drag scrolling.
    pub(crate) fn try_minimap_left_down(&mut self, x: i32, y: i32) -> bool {
        if !self.scroll_to_minimap_point(x, y, true) {
            return false;
        }
        self.mouse_state.minimap_dragging = true;
        self.mouse_state.dragging = true;
        unsafe {
            let _ = SetCapture(self.hwnd);
        }
        true
    }

    /// `WM_MOUSEMOVE` while the minimap drag is active.
    pub(crate) fn try_minimap_drag_mouse_move(&mut self, x: i32, y: i32) -> bool {
        if !self.mouse_state.minimap_dragging {
            return false;
        }
        let _ = self.scroll_to_minimap_point(x, y, false);
        true
    }

    /// `WM_LBUTTONUP` terminates an active minimap drag.
    pub(crate) fn try_minimap_left_up(&mut self) -> bool {
        if !self.mouse_state.minimap_dragging {
            return false;
        }
        self.mouse_state.minimap_dragging = false;
        unsafe {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
        }
        self.request_state_save();
        true
    }

    fn scroll_to_minimap_point(&mut self, x: i32, y: i32, should_trace: bool) -> bool {
        if !self.view_options.minimap {
            return false;
        }
        let Some(layout) = self.view_options.minimap_layout.clone() else {
            return false;
        };
        let body = self.focused_body_rect();
        let xf = x as f32;
        let yf = y as f32;
        if yf < body.y || yf >= body.y + body.h {
            return false;
        }
        let Some(hit) = continuity_render::minimap_hit_test(&layout, xf - body.x, yf - body.y)
        else {
            return false;
        };
        let content_height_dip = self.estimated_content_height();
        let line_height = self.effective_line_height();
        let target_buffer_y = hit.line as f32 * line_height;
        let target_dip = compute_minimap_target_scroll(
            hit.line,
            self.view.viewport_height_dip,
            line_height,
            content_height_dip,
        );
        let before = self.view.scroll_y_dip;
        self.view.jump_to(target_dip, content_height_dip);
        let scrolled = (self.view.scroll_y_dip - before).abs() > f32::EPSILON;
        if should_trace && crate::paint_trace::is_trace_enabled() {
            crate::paint_trace::log_event(
                "event:minimap_click",
                &format!(
                    "target_dip={:.1} target_buffer_y={:.1} scrolled={}",
                    target_dip, target_buffer_y, scrolled
                ),
            );
        }
        if scrolled {
            invalidate_hwnd(self.hwnd);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimap_target_scroll_centers_clicked_line() {
        let target = compute_minimap_target_scroll(100, 400.0, 20.0, 4_000.0);
        assert_eq!(target, 1_800.0);
    }

    #[test]
    fn minimap_target_scroll_clamps_to_scroll_range() {
        assert_eq!(compute_minimap_target_scroll(1, 400.0, 20.0, 4_000.0), 0.0);
        assert_eq!(
            compute_minimap_target_scroll(1_000, 400.0, 20.0, 4_000.0),
            3_600.0
        );
    }
}
