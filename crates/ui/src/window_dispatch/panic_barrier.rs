//! Panic-recovery helpers for the Win32 [`crate::window_dispatch::wndproc`]
//! FFI barrier.
//!
//! `wndproc` is invoked by Win32 across an `extern "system"` boundary.
//! An unwind crossing that boundary is undefined behavior that aborts
//! the process, and the shipped `release-small` profile compiles with
//! `panic = "unwind"`, so a panic in any paint/dispatch path would tear
//! the whole app down. The barrier in `window_dispatch.rs` wraps the
//! routing body in [`std::panic::catch_unwind`]; this module owns the
//! caught-panic recovery: logging plus choosing a safe `LRESULT` so the
//! message pump survives a single faulting message.
//!
//! Thread ownership: invoked only from the UI thread that owns the
//! faulting `Window`, on the recovery path of its message pump.

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, WM_CAPTURECHANGED, WM_CHAR, WM_CLOSE, WM_DESTROY, WM_DPICHANGED, WM_DROPFILES,
    WM_IME_STARTCOMPOSITION, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MBUTTONDOWN, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_SYSCHAR, WM_TIMER,
};

/// Recovery path for a panic caught at the
/// [`crate::window_dispatch::wndproc`] FFI barrier.
///
/// Logs the panic (message + offending `HWND`/`WM_*`) and returns a
/// safe `LRESULT` so the message pump keeps running:
/// - For messages the app treats as "handled" (returning
///   `Some(LRESULT(0))` in `Window::handle_message`), `LRESULT(0)` is
///   the correct quiescent reply.
/// - For every other message, hand off to `DefWindowProcW` so the OS
///   default behavior still applies rather than silently swallowing it.
///
/// `DefWindowProcW` here never touches the (possibly inconsistent)
/// `Window` state, so it is safe to call even after a mid-mutation
/// panic.
pub(crate) fn recover_from_wndproc_panic(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    payload: Box<dyn std::any::Any + Send>,
) -> LRESULT {
    let message = panic_payload_message(payload.as_ref());
    eprintln!(
        "wndproc panic recovered: hwnd={:?} msg=0x{msg:04X} message={message}",
        hwnd.0
    );
    if is_handled_message(msg) {
        LRESULT(0)
    } else {
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }
}

/// Extracts a human-readable string from a caught panic payload, which
/// is `&str` for `panic!("literal")` and `String` for formatted panics.
fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> &str {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        s
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.as_str()
    } else {
        "<non-string panic payload>"
    }
}

/// Whether `Window::handle_message` treats `msg` as handled (returns
/// `Some(LRESULT(0))`) on the normal path. Used by the panic-recovery
/// barrier to choose between a quiescent `LRESULT(0)` and a
/// `DefWindowProcW` fall-through. Mirrors the dispatch arms in
/// `window_dispatch.rs`; the conservative default for anything not
/// listed is "unhandled", so a new arm that forgets to update this
/// falls through to the OS default rather than swallowing the message.
fn is_handled_message(msg: u32) -> bool {
    matches!(
        msg,
        WM_PAINT
            | WM_CHAR
            | WM_IME_STARTCOMPOSITION
            | WM_KEYDOWN
            | WM_KEYUP
            | WM_SYSCHAR
            | WM_LBUTTONDOWN
            | WM_LBUTTONDBLCLK
            | WM_LBUTTONUP
            | WM_MOUSEMOVE
            | WM_MOUSELEAVE
            | WM_CAPTURECHANGED
            | WM_MBUTTONDOWN
            | WM_MOUSEWHEEL
            | WM_DROPFILES
            | WM_TIMER
            | WM_DPICHANGED
            | WM_CLOSE
            | WM_DESTROY
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handled_message_set_covers_zero_lresult_arms() {
        // Messages whose dispatch arm returns Some(LRESULT(0)).
        for msg in [
            WM_PAINT,
            WM_CHAR,
            WM_IME_STARTCOMPOSITION,
            WM_KEYDOWN,
            WM_KEYUP,
            WM_SYSCHAR,
            WM_LBUTTONDOWN,
            WM_LBUTTONDBLCLK,
            WM_LBUTTONUP,
            WM_MOUSEMOVE,
            WM_MOUSELEAVE,
            WM_CAPTURECHANGED,
            WM_MBUTTONDOWN,
            WM_MOUSEWHEEL,
            WM_DROPFILES,
            WM_TIMER,
            WM_DPICHANGED,
            WM_CLOSE,
            WM_DESTROY,
        ] {
            assert!(
                is_handled_message(msg),
                "msg 0x{msg:04X} should be treated as handled"
            );
        }
    }

    #[test]
    fn unhandled_messages_fall_through() {
        use windows::Win32::UI::WindowsAndMessaging::{
            WM_ERASEBKGND, WM_MOVE, WM_SETTINGCHANGE, WM_SIZE,
        };
        // Arms that return None or a non-zero LRESULT take the
        // DefWindowProcW fall-through on recovery, not LRESULT(0).
        for msg in [
            WM_ERASEBKGND,
            WM_SIZE,
            WM_MOVE,
            WM_SETTINGCHANGE,
            0xDEAD_u32,
        ] {
            assert!(
                !is_handled_message(msg),
                "msg 0x{msg:04X} should fall through to DefWindowProcW"
            );
        }
    }

    #[test]
    fn payload_message_reads_str_literal_panic() {
        let payload: Box<dyn std::any::Any + Send> =
            std::panic::catch_unwind(|| panic!("boom literal")).expect_err("closure must panic");
        assert_eq!(panic_payload_message(payload.as_ref()), "boom literal");
    }

    #[test]
    fn payload_message_reads_formatted_panic() {
        let code = 42;
        let payload: Box<dyn std::any::Any + Send> =
            std::panic::catch_unwind(|| panic!("boom {code}")).expect_err("closure must panic");
        assert_eq!(panic_payload_message(payload.as_ref()), "boom 42");
    }

    #[test]
    fn payload_message_handles_non_string_payload() {
        let payload: Box<dyn std::any::Any + Send> =
            std::panic::catch_unwind(|| std::panic::panic_any(7u32))
                .expect_err("closure must panic");
        assert_eq!(
            panic_payload_message(payload.as_ref()),
            "<non-string panic payload>"
        );
    }
}
