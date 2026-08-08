//! Drag-resize behavior for the left file-tree pane.

use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};

use crate::mouse::FileTreeResizeDrag;
use crate::window::Window;
use crate::window_helpers::invalidate_hwnd;

const GRAB_BAND_HALF_DIP: f32 = 4.0;

impl Window {
    pub(crate) fn cursor_over_file_tree_resize_band(&self, x: f32, y: f32) -> bool {
        self.file_tree.is_visible()
            && y >= 0.0
            && y < self.pane_root_rect().h
            && (x - self.file_tree.visible_width_dip()).abs() <= GRAB_BAND_HALF_DIP
    }

    pub(crate) fn try_file_tree_resize_left_down(&mut self, x: i32, y: i32) -> bool {
        if !self.cursor_over_file_tree_resize_band(x as f32, y as f32) {
            return false;
        }
        self.mouse_state.file_tree_resize_drag = Some(FileTreeResizeDrag {
            start_x: x,
            start_width_dip: self.file_tree.width_dip(),
        });
        if !self.hwnd.0.is_null() {
            unsafe {
                let _ = SetCapture(self.hwnd);
            }
        }
        true
    }

    pub(crate) fn drag_file_tree_resize(&mut self, x: i32) -> bool {
        let Some(drag) = self.mouse_state.file_tree_resize_drag else {
            return false;
        };
        let before = self.file_tree.width_dip();
        self.file_tree
            .set_width_dip(drag.start_width_dip + (x - drag.start_x) as f32);
        if (before - self.file_tree.width_dip()).abs() < 0.5 {
            return false;
        }
        self.clear_right_edge_layout_caches();
        self.refresh_focused_viewport();
        invalidate_hwnd(self.hwnd);
        true
    }

    pub(crate) fn finish_file_tree_resize(&mut self) -> bool {
        if self.mouse_state.file_tree_resize_drag.take().is_none() {
            return false;
        }
        unsafe {
            let _ = ReleaseCapture();
        }
        self.request_state_save();
        self.persist_vault_workspace_state();
        let _ = self.try_dispatch_projection_worker_early("file_tree_resize_end", "layout_change");
        true
    }
}
