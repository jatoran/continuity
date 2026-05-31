//! Pretty-name lookup for Win32 message ids used by the
//! `wndproc` trace lines.
//!
//! Lifted out of `paint_trace.rs` to keep that file under the
//! 600-line conventions cap. Pure: no shared state, no I/O.

/// Pretty name for a wndproc message id. Covers the ones we time
/// individually; everything else is logged as `WM_<hex>`.
pub(crate) fn wndproc_message_name(msg: u32) -> String {
    use windows::Win32::UI::WindowsAndMessaging::{
        WM_ACTIVATE, WM_ACTIVATEAPP, WM_CHAR, WM_CLOSE, WM_CREATE, WM_DESTROY, WM_DPICHANGED,
        WM_DROPFILES, WM_ENTERSIZEMOVE, WM_ERASEBKGND, WM_EXITSIZEMOVE, WM_KEYDOWN, WM_KEYUP,
        WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_MOVE,
        WM_NCCALCSIZE, WM_NCHITTEST, WM_NCPAINT, WM_PAINT, WM_RBUTTONDOWN, WM_RBUTTONUP,
        WM_SETCURSOR, WM_SETFOCUS, WM_SIZE, WM_SIZING, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER,
        WM_WINDOWPOSCHANGED, WM_WINDOWPOSCHANGING,
    };
    match msg {
        WM_PAINT => "WM_PAINT".to_string(),
        WM_TIMER => "WM_TIMER".to_string(),
        WM_KEYDOWN => "WM_KEYDOWN".to_string(),
        WM_KEYUP => "WM_KEYUP".to_string(),
        WM_CHAR => "WM_CHAR".to_string(),
        WM_SYSKEYDOWN => "WM_SYSKEYDOWN".to_string(),
        WM_SYSKEYUP => "WM_SYSKEYUP".to_string(),
        WM_MOUSEMOVE => "WM_MOUSEMOVE".to_string(),
        WM_LBUTTONDOWN => "WM_LBUTTONDOWN".to_string(),
        WM_LBUTTONUP => "WM_LBUTTONUP".to_string(),
        WM_RBUTTONDOWN => "WM_RBUTTONDOWN".to_string(),
        WM_RBUTTONUP => "WM_RBUTTONUP".to_string(),
        WM_MOUSEWHEEL => "WM_MOUSEWHEEL".to_string(),
        WM_SETFOCUS => "WM_SETFOCUS".to_string(),
        WM_KILLFOCUS => "WM_KILLFOCUS".to_string(),
        WM_ACTIVATE => "WM_ACTIVATE".to_string(),
        WM_ACTIVATEAPP => "WM_ACTIVATEAPP".to_string(),
        WM_SIZE => "WM_SIZE".to_string(),
        WM_MOVE => "WM_MOVE".to_string(),
        WM_DPICHANGED => "WM_DPICHANGED".to_string(),
        WM_ENTERSIZEMOVE => "WM_ENTERSIZEMOVE".to_string(),
        WM_EXITSIZEMOVE => "WM_EXITSIZEMOVE".to_string(),
        WM_ERASEBKGND => "WM_ERASEBKGND".to_string(),
        WM_NCPAINT => "WM_NCPAINT".to_string(),
        WM_SIZING => "WM_SIZING".to_string(),
        WM_WINDOWPOSCHANGING => "WM_WINDOWPOSCHANGING".to_string(),
        WM_WINDOWPOSCHANGED => "WM_WINDOWPOSCHANGED".to_string(),
        WM_SETCURSOR => "WM_SETCURSOR".to_string(),
        WM_NCCALCSIZE => "WM_NCCALCSIZE".to_string(),
        WM_NCHITTEST => "WM_NCHITTEST".to_string(),
        WM_CLOSE => "WM_CLOSE".to_string(),
        WM_DESTROY => "WM_DESTROY".to_string(),
        WM_CREATE => "WM_CREATE".to_string(),
        WM_DROPFILES => "WM_DROPFILES".to_string(),
        _ => format!("WM_{msg:#06x}"),
    }
}
