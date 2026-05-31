//! Paint-tail Win32 validation and post-frame UI nudges.

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};

use crate::window::Window;

use super::doc_end_scroll::DocEndSnapPaintAction;

impl Window {
    pub(crate) fn finish_paint_epilogue(
        &mut self,
        hwnd: HWND,
        should_start_motion_timer: bool,
        doc_end_snap_action: DocEndSnapPaintAction,
    ) {
        if should_start_motion_timer {
            self.start_motion_timer();
        }
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let _ = BeginPaint(hwnd, &mut ps);
            let _ = EndPaint(hwnd, &ps);
        }
        if doc_end_snap_action.post_paint_invalidate {
            self.invalidate_with_reason(hwnd, "doc_end_snap");
        }
        if !self.inited {
            self.start_caret_blink(hwnd);
        }
        self.inited = true;
        // Keep nudging while deferred font-swap spectator frames catch up.
        self.nudge_font_swap_settle();
    }
}
