//! Process DPI awareness configuration.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::HiDpi::{
    GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

use crate::Error;

/// Opt the process into per-monitor DPI v2 awareness.
///
/// Idempotent (`SetProcessDpiAwarenessContext` returns false if already set,
/// which we treat as success here). This must be called *before* the first
/// HWND is created.
///
/// # Errors
///
/// Returns [`Error::Win32`] if the API itself fails for a reason other than
/// "already set".
pub fn set_per_monitor_dpi_v2() -> Result<(), Error> {
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_err() {
            // Already set is the typical case in tests where multiple test
            // threads share a process. Treat as success.
            let last = windows::core::Error::from_win32();
            // ERROR_ACCESS_DENIED (0x80070005) is what Windows returns when
            // the awareness was already set. Anything else is a real failure.
            if last.code().0 as u32 != 0x8007_0005 {
                return Err(Error::win32("SetProcessDpiAwarenessContext", last));
            }
        }
    }
    Ok(())
}

/// Return the current DPI for `hwnd`, clamped to the Win32 default
/// when the handle is not yet associated with a real window.
#[must_use]
pub fn dpi_for_window(hwnd: HWND) -> u32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    dpi.max(96)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_dpi_v2_is_idempotent() {
        set_per_monitor_dpi_v2().unwrap();
        set_per_monitor_dpi_v2().unwrap();
    }
}
