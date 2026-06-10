//! Outline-sidebar drag-resize: grab band on the sidebar's left edge,
//! live width updates while dragging, and width persistence to
//! `[ui].outline_sidebar_width_dip` on release.
//!
//! Thread ownership: all state touched here (`mouse_state`,
//! `view_options.outline_sidebar_width_dip`) is owned by the window's
//! UI thread; every entry point runs inside the wndproc.

use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};

use crate::mouse::OutlineResizeDrag;
use crate::window_helpers::invalidate_hwnd;
use crate::Window;

/// Width of the grab band centered on the sidebar's left edge, DIPs.
const GRAB_BAND_HALF_DIP: f32 = 4.0;
/// Narrowest useful sidebar.
const MIN_WIDTH_DIP: f32 = 120.0;
/// Widest sidebar, also clamped to 80 % of the focused body below.
const MAX_WIDTH_DIP: f32 = 600.0;

impl Window {
    /// `true` when `(xf, yf)` sits on the sidebar's left-edge grab band.
    pub(crate) fn cursor_over_outline_resize_band(&self, xf: f32, yf: f32) -> bool {
        let Some((left, top, _, height)) = self.outline_sidebar_rect() else {
            return false;
        };
        yf >= top && yf <= top + height && (xf - left).abs() <= GRAB_BAND_HALF_DIP
    }

    /// `WM_LBUTTONDOWN` on the grab band begins the resize drag.
    pub(crate) fn try_outline_resize_left_down(&mut self, x: i32, y: i32) -> bool {
        if !self.cursor_over_outline_resize_band(x as f32, y as f32) {
            return false;
        }
        self.mouse_state.outline_resize_drag = Some(OutlineResizeDrag {
            start_x: x,
            start_width_dip: self.view_options.outline_sidebar_width_dip,
        });
        if !self.hwnd.0.is_null() {
            unsafe {
                let _ = SetCapture(self.hwnd);
            }
        }
        true
    }

    /// Drive an in-flight resize on `WM_MOUSEMOVE`. Returns `true` when
    /// the width changed so the caller invalidates.
    pub(crate) fn drag_outline_resize(&mut self, x: i32) -> bool {
        let Some(drag) = self.mouse_state.outline_resize_drag else {
            return false;
        };
        // The grabbed edge is the sidebar's LEFT edge: moving the
        // pointer left widens the right-docked sidebar.
        let proposed = drag.start_width_dip + (drag.start_x - x) as f32;
        let body = self.focused_body_rect();
        let max = MAX_WIDTH_DIP.min((body.w * 0.8).max(MIN_WIDTH_DIP));
        let next = proposed.clamp(MIN_WIDTH_DIP, max);
        if (next - self.view_options.outline_sidebar_width_dip).abs() < 0.5 {
            return false;
        }
        self.view_options.outline_sidebar_width_dip = next;
        invalidate_hwnd(self.hwnd);
        true
    }

    /// `WM_LBUTTONUP` ends the drag: release capture, persist the new
    /// width, and prewarm the projection at the settled wrap width
    /// (the sidebar consumes body width, so resizing reflows text).
    pub(crate) fn finish_outline_resize(&mut self) -> bool {
        if self.mouse_state.outline_resize_drag.take().is_none() {
            return false;
        }
        unsafe {
            let _ = ReleaseCapture();
        }
        let width = self.view_options.outline_sidebar_width_dip.round() as u32;
        self.persist_int_or_log("ui", "outline_sidebar_width_dip", width);
        let _ = self.try_dispatch_projection_worker_early("outline_resize_end", "layout_change");
        true
    }

    /// Outer rect of the visible outline sidebar `(x, y, w, h)` in
    /// client DIPs, from the paint-cached layout when available, else
    /// derived from the focused body rect (matches the painter's
    /// right-docked placement).
    fn outline_sidebar_rect(&self) -> Option<(f32, f32, f32, f32)> {
        if !self.view_options.show_outline_sidebar {
            return None;
        }
        if let Some(layout) = self.view_options.outline_layout.as_ref() {
            return Some(layout.rect);
        }
        let body = self.focused_body_rect();
        let width = self
            .view_options
            .outline_sidebar_width_dip
            .max(0.0)
            .min(body.w);
        if width <= 0.0 {
            return None;
        }
        Some((body.x + body.w - width, body.y, width, body.h))
    }
}
