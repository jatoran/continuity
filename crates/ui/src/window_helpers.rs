//! Tiny pure helpers used by [`crate::Window`]'s message routing — kept
//! out of `window.rs` so that file stays under the 600-line cap.

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::Graphics::Gdi::InvalidateRect;

use continuity_win::ComGuard;

/// Decode an `LPARAM` carrying packed `(x, y)` coordinates from a Win32
/// mouse message.
pub(crate) fn lparam_to_xy(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam.0 as i32) & 0xFFFF;
    let y = ((lparam.0 as i32) >> 16) & 0xFFFF;
    (sign_extend_16(x), sign_extend_16(y))
}

/// Sign-extend a 16-bit value packed into the low half of an i32.
pub(crate) fn sign_extend_16(v: i32) -> i32 {
    if v & 0x8000 != 0 {
        v - 0x1_0000
    } else {
        v
    }
}

/// Mark the entire client area of `hwnd` for repaint without erasing
/// the background. Shared by every command handler that mutates state
/// the renderer reads from.
pub(crate) fn invalidate_hwnd(hwnd: HWND) {
    invalidate_hwnd_with_reason(hwnd, "invalidate_rect");
}

/// Mark the entire client area of `hwnd` for repaint with a trace
/// reason but without requiring a full [`crate::Window`] borrow.
pub(crate) fn invalidate_hwnd_with_reason(hwnd: HWND, reason: &'static str) {
    crate::paint_trace::note_invalidate_request_with_reason(reason);
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

// Touch one symbol from `continuity_win` so the dep is exercised; the
// actual COM init lives in `app::main`.
const _: fn() = || {
    let _ = std::mem::size_of::<ComGuard>();
};
