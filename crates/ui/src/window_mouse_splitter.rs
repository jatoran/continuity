//! Splitter mouse interactions split from [`crate::window_mouse`].
//!
//! Covers splitter double-click equalize, drag start, and in-flight
//! drag ratio updates. These methods mutate only `Window`'s UI-thread
//! pane tree and mouse state.

use windows::Win32::UI::Input::KeyboardAndMouse::SetCapture;

use crate::mouse::SplitterDrag;
use crate::pane_splitter::{splitters, Splitter};
use crate::pane_tree::SplitAxis;
use crate::Window;

impl Window {
    /// `WM_LBUTTONDBLCLK` on a splitter equalizes the enclosing split's
    /// ratios. Returns true when consumed.
    pub(crate) fn try_splitter_dbl_click(&mut self, x: i32, y: i32) -> bool {
        let xf = x as f32;
        let yf = y as f32;
        let root = self.pane_root_rect();
        let hit = splitters(&self.tree, root)
            .into_iter()
            .find(|s| s.hit.contains(xf, yf));
        let Some(s) = hit else {
            return false;
        };
        if crate::pane_splitter::equalize_split_for(&mut self.tree, s.left_leaf, s.axis) {
            self.request_state_save();
            return true;
        }
        false
    }

    /// `WM_LBUTTONDOWN` on a splitter begins a drag-resize.
    pub(crate) fn try_splitter_left_down(&mut self, x: i32, y: i32) -> bool {
        let xf = x as f32;
        let yf = y as f32;
        let root = self.pane_root_rect();
        let hit: Option<Splitter> = splitters(&self.tree, root)
            .into_iter()
            .find(|s| s.hit.contains(xf, yf));
        let Some(s) = hit else {
            return false;
        };
        self.mouse_state.dragging = true;
        self.mouse_state.splitter_drag = Some(SplitterDrag {
            axis: s.axis,
            left_leaf: s.left_leaf,
            start_x: x,
            start_y: y,
            root_w: root.w,
            root_h: root.h,
        });
        if self.hwnd.0 as isize != 0 {
            unsafe {
                let _ = SetCapture(self.hwnd);
            }
        }
        true
    }

    /// Drive an in-flight splitter drag on `WM_MOUSEMOVE`. Returns true
    /// when the ratio actually shifted so the caller can invalidate.
    pub(crate) fn drag_splitter(&mut self, x: i32, y: i32) -> bool {
        let Some(drag) = self.mouse_state.splitter_drag else {
            return false;
        };
        let (delta_dip, root_dim) = match drag.axis {
            SplitAxis::Horizontal => ((x - drag.start_x) as f32, drag.root_w),
            SplitAxis::Vertical => ((y - drag.start_y) as f32, drag.root_h),
        };
        if root_dim <= 0.0 || delta_dip == 0.0 {
            return false;
        }
        let prev_focused = self.tree.focused;
        self.tree.focused = drag.left_leaf;
        crate::pane_layout::resize_focused(&mut self.tree, drag.axis, delta_dip, root_dim);
        self.tree.focused = prev_focused;
        let drag_mut = self
            .mouse_state
            .splitter_drag
            .as_mut()
            .expect("invariant: splitter_drag present after the early return above");
        drag_mut.start_x = x;
        drag_mut.start_y = y;
        true
    }
}
