//! Mouse cursor selection for [`crate::Window`].

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, LoadCursorW, SetCursor, IDC_ARROW, IDC_HAND, IDC_IBEAM, IDC_SIZENS, IDC_SIZEWE,
};

use crate::pane_layout::metrics;
use crate::window_overlay_input::OverlayCursor;
use crate::Window;

impl Window {
    /// `WM_SETCURSOR`: choose I-beam over the editor body and the default
    /// arrow over non-text chrome / tab strips. Returns `true` when we set
    /// a cursor (caller should return `TRUE` to halt default processing).
    pub(crate) fn on_set_cursor(&self, hwnd: HWND) -> bool {
        let mut pt = POINT::default();
        if unsafe { GetCursorPos(&mut pt) }.is_err() {
            return false;
        }
        let ok = unsafe { ScreenToClient(hwnd, &mut pt).as_bool() };
        if !ok {
            return false;
        }
        let xf = pt.x as f32;
        let yf = pt.y as f32;
        if let Some(overlay_cursor) = self.overlay_cursor_at(xf, yf) {
            let name = match overlay_cursor {
                OverlayCursor::Arrow => IDC_ARROW,
                OverlayCursor::Hand => IDC_HAND,
                OverlayCursor::IBeam => IDC_IBEAM,
            };
            if let Ok(cursor) = unsafe { LoadCursorW(None, name) } {
                unsafe { SetCursor(Some(cursor)) };
                return true;
            }
        }
        if self.cursor_over_non_text_chrome(xf, yf) {
            if let Ok(cursor) = unsafe { LoadCursorW(None, IDC_ARROW) } {
                unsafe { SetCursor(Some(cursor)) };
            }
            return true;
        }
        if self.focused_tab_is_buffer_history() {
            let body = self.focused_body_rect();
            if xf >= body.x && xf <= body.x + body.w && yf >= body.y && yf <= body.y + body.h {
                if let Ok(cursor) = unsafe { LoadCursorW(None, IDC_ARROW) } {
                    unsafe { SetCursor(Some(cursor)) };
                }
                return true;
            }
        }
        let splitter_axis = crate::pane_splitter::splitters(&self.tree, self.pane_root_rect())
            .into_iter()
            .find(|s| s.hit.contains(xf, yf))
            .map(|s| s.axis);
        let in_strip = self
            .pane_outer_rects()
            .into_iter()
            .any(|(_, r)| r.contains(xf, yf) && yf < r.y + metrics::TAB_STRIP_HEIGHT_DIP);
        let ctrl_held = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
        let over_scrollbar = self.cursor_over_scrollbar(xf, yf);
        let over_code_copy = self.cursor_over_code_copy_button(xf, yf);
        let over_table_col_border =
            !in_strip && !over_scrollbar && self.cursor_over_table_col_border(pt.x, pt.y);
        let name = match splitter_axis {
            Some(crate::pane_tree::SplitAxis::Horizontal) => IDC_SIZEWE,
            Some(crate::pane_tree::SplitAxis::Vertical) => IDC_SIZENS,
            None if self.cursor_over_outline_resize_band(xf, yf) => IDC_SIZEWE,
            None if in_strip => IDC_ARROW,
            None if over_scrollbar => IDC_ARROW,
            None if over_code_copy => IDC_HAND,
            None if over_table_col_border => IDC_SIZEWE,
            None if self.cursor_over_open_link(pt.x, pt.y) => IDC_HAND,
            None if ctrl_held && self.cursor_over_ctrl_click_target(pt.x, pt.y) => IDC_HAND,
            None if self.cursor_over_checkbox(pt.x, pt.y) => IDC_ARROW,
            None if self.cursor_over_line_number_gutter(xf, yf) => IDC_ARROW,
            None => IDC_IBEAM,
        };
        let Ok(cursor) = (unsafe { LoadCursorW(None, name) }) else {
            return false;
        };
        unsafe {
            SetCursor(Some(cursor));
        }
        true
    }

    /// `true` when `(xf, yf)` (client coords) is over the focused pane's
    /// line-number gutter — the column holding the line numbers and the
    /// collapse/expand fold icons. Over it the cursor shows a normal
    /// arrow instead of the text I-beam. The width tracks
    /// [`continuity_render::chrome::gutter_width_for_line_count`], so the
    /// hit region adapts to font size and to the buffer's line count
    /// exactly like the painted gutter.
    fn cursor_over_line_number_gutter(&self, xf: f32, yf: f32) -> bool {
        if !self.view_options.line_numbers {
            return false;
        }
        let body = self.focused_body_rect();
        if yf < body.y || yf >= body.y + body.h || xf < body.x {
            return false;
        }
        let source_line_count = self
            .editor
            .snapshot(self.buffer_id)
            .map(|snapshot| snapshot.rope_snapshot().rope().len_lines())
            .unwrap_or(1);
        let gutter_width = continuity_render::chrome::gutter_width_for_line_count(
            self.scaled_font_size(),
            source_line_count,
        );
        xf < body.x + gutter_width
    }
}
