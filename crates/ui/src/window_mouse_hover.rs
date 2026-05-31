//! Line-hover detection + a tiny wall-clock helper used by the mouse
//! click pipeline. Split out of `window_mouse.rs` to keep that file
//! under the 600-line cap.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};

use crate::Window;

impl Window {
    /// Arm a one-shot `WM_MOUSELEAVE` notification for this HWND.
    pub(crate) fn ensure_mouse_leave_tracking(&mut self, hwnd: HWND) {
        if self.mouse_state.mouse_leave_tracking || hwnd.0 as isize == 0 {
            return;
        }
        let mut event = TRACKMOUSEEVENT {
            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: hwnd,
            dwHoverTime: 0,
        };
        if unsafe { TrackMouseEvent(&mut event) }.is_ok() {
            self.mouse_state.mouse_leave_tracking = true;
        }
    }

    /// Clear transient pointer-hover state when the cursor leaves the HWND.
    pub(crate) fn on_mouse_leave(&mut self) -> bool {
        self.mouse_state.mouse_leave_tracking = false;
        let mut changed = false;
        changed |= self.mouse_state.line_hover.take().is_some();
        changed |= std::mem::take(&mut self.mouse_state.gutter_hovered);
        changed |= self.clear_tab_hover();
        changed |= self.clear_footnote_hover();
        changed |= self.clear_code_copy_hover();
        changed
    }

    /// Update hovered-line and gutter-hover state from a fresh client point.
    pub(crate) fn update_line_hover_from_pixel(&mut self, x: i32, y: i32) -> bool {
        let previous_hover = self.mouse_state.line_hover;
        let previous_gutter = self.mouse_state.gutter_hovered;
        let next_hover = self.compute_line_hover_from_pixel(x, y);
        self.mouse_state.gutter_hovered = next_hover.is_some_and(|hover| hover.in_gutter);
        self.mouse_state.line_hover = next_hover;
        previous_hover != self.mouse_state.line_hover
            || previous_gutter != self.mouse_state.gutter_hovered
    }

    fn compute_line_hover_from_pixel(
        &self,
        x: i32,
        y: i32,
    ) -> Option<crate::mouse::MouseLineHover> {
        let body = self.focused_body_rect();
        let xf = x as f32;
        let yf = y as f32;
        if xf < body.x || xf >= body.x + body.w || yf < body.y || yf >= body.y + body.h {
            return None;
        }
        let display_row = self.display_row_for_client_y(y);
        let (_, frame_display) = self.last_painted_frame_display.as_ref()?;
        let (source_line, _) = frame_display
            .row_index()
            .source_line_for_display_row(display_row)?;
        let source_line_count = self
            .editor
            .snapshot(self.buffer_id)
            .map(|snapshot| snapshot.rope_snapshot().rope().len_lines())
            .unwrap_or(1);
        let gutter_width = continuity_render::chrome::gutter_width_for_line_count(
            self.scaled_font_size(),
            source_line_count,
        );
        let in_gutter =
            self.view_options.line_numbers && xf >= body.x && xf < body.x + gutter_width;
        Some(crate::mouse::MouseLineHover {
            source_line: source_line.raw(),
            display_row,
            in_gutter,
        })
    }
}

/// Monotonic-ish millis since the Unix epoch — used to drive click-
/// count detection (single / double / triple). Returns `0` on the
/// degenerate clock-failure case so the click classifier still produces
/// a stable answer rather than panicking.
pub(crate) fn wall_clock_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
