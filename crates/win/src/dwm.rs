//! DWM (Desktop Window Manager) window-attribute wrappers.
//!
//! Tier-1 OS title-bar theming: toggle the system-drawn caption (title
//! text, the min/max/close buttons, and the window frame) between the
//! light and dark immersive palettes via `DWMWA_USE_IMMERSIVE_DARK_MODE`.
//! Arbitrary caption colors (`DWMWA_CAPTION_COLOR`, Windows 11 only) are
//! intentionally out of scope here.

use windows::Win32::Foundation::{BOOL, HWND};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};

use crate::Error;

/// Toggle the OS-drawn title bar between the dark and light immersive
/// palettes.
///
/// Backed by the `DWMWA_USE_IMMERSIVE_DARK_MODE` window attribute, which
/// exists on Windows 10 build 1809+ and every Windows 11 build. On older
/// builds the attribute is unsupported and the call returns an error —
/// there is simply no OS dark caption to set, so callers may treat the
/// error as "nothing to do".
///
/// Must be called on the thread that owns `hwnd`.
///
/// # Errors
///
/// Returns [`Error::Win32`] when `DwmSetWindowAttribute` fails, including
/// the "attribute unsupported" case on pre-1809 Windows.
pub fn set_titlebar_dark_mode(hwnd: HWND, dark: bool) -> Result<(), Error> {
    let value = BOOL::from(dark);
    let size = u32::try_from(std::mem::size_of::<BOOL>()).unwrap_or(4);
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            std::ptr::addr_of!(value).cast(),
            size,
        )
    }
    .map_err(|e| Error::win32("DwmSetWindowAttribute", e))
}
