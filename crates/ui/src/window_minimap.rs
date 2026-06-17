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
        // §28 — resolve the click in display-row space so it matches the
        // editor scroll under soft-wrap. The hit carries a pre-clamped
        // target scroll computed proportionally against the same
        // content-height/viewport pair the editor clamps against.
        let content_height_dip = self.estimated_content_height();
        let Some(hit) = continuity_render::minimap_hit_test(
            &layout,
            xf - body.x,
            yf - body.y,
            content_height_dip,
            self.view.viewport_height_dip,
        ) else {
            return false;
        };
        let line_height = self.effective_line_height();
        let target_buffer_y = hit.line as f32 * line_height;
        let target_dip = hit.target_scroll_dip;
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
    use continuity_render::compute_minimap_layout;

    /// §28 — the scroll target now comes from
    /// [`continuity_render::minimap_hit_test`], resolved in display-row
    /// space. A click at the vertical midpoint of the strip centers the
    /// viewport on the middle of the (display-row) content height.
    #[test]
    fn midpoint_click_centers_viewport_on_content() {
        let content_h = 4_000.0;
        let layout =
            compute_minimap_layout((0.0, 0.0, 800.0, 600.0), 0.0, 20.0, 200, content_h, 0.0);
        let (rx, ry, rw, rh) = layout.rect;
        let hit = continuity_render::minimap_hit_test(
            &layout,
            rx + rw * 0.5,
            ry + rh * 0.5,
            content_h,
            400.0,
        )
        .expect("click lands in strip");
        let expected = (content_h * 0.5 - 200.0).clamp(0.0, content_h - 400.0);
        assert!((hit.target_scroll_dip - expected).abs() < 1.0);
    }

    #[test]
    fn click_outside_strip_misses() {
        let content_h = 4_000.0;
        let layout =
            compute_minimap_layout((0.0, 0.0, 800.0, 600.0), 0.0, 20.0, 200, content_h, 0.0);
        let (rx, ry, _, _) = layout.rect;
        assert!(continuity_render::minimap_hit_test(
            &layout,
            rx - 10.0,
            ry + 50.0,
            content_h,
            400.0
        )
        .is_none());
    }
}
