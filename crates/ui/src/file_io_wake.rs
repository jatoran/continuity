//! Event-driven wake for routed file-I/O completions.

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_TIMER};

/// Ask the destination window to drain its file-I/O reply queue now.
pub(crate) fn wake_file_io_window(raw_window: Option<usize>) {
    let Some(raw_window) = raw_window.filter(|raw_window| *raw_window != 0) else {
        return;
    };
    let hwnd = HWND(raw_window as *mut core::ffi::c_void);
    unsafe {
        let _ = PostMessageW(
            Some(hwnd),
            WM_TIMER,
            WPARAM(crate::window_timers::FILE_IO_TIMER_ID),
            LPARAM(0),
        );
    }
}
