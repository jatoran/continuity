//! Embedded application-icon loading.
//!
//! Loads the icon compiled into the executable's resource table (resource
//! id 1, matching `crates/app/assets/continuity.rc`) so callers can attach
//! it to a window's caption / Alt-Tab entry via `WM_SETICON`.
//!
//! The icon is loaded with `LR_SHARED`: the OS owns the handle's lifetime
//! and reuses a single cached `HICON` per (module, size) for the process,
//! so the returned handle must NOT be passed to `DestroyIcon`. There is no
//! tracked state and nothing to clean up.

use windows::core::PCWSTR;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, LoadImageW, HICON, IMAGE_ICON, LR_DEFAULTSIZE, LR_SHARED, SM_CXICON,
    SM_CXSMICON, SM_CYICON, SM_CYSMICON,
};

use crate::Error;

/// Resource id of the embedded application icon. Matches the single
/// `1 ICON "assets\\continuity.ico"` line in `crates/app/assets/continuity.rc`,
/// which `crates/app/build.rs` compiles into the executable.
const APP_ICON_RESOURCE_ID: u16 = 1;

/// Load the embedded application icon from the running executable's
/// resource table.
///
/// `large` selects which system metric the icon is scaled to: the big
/// (`SM_CXICON` / `SM_CYICON`) caption / Alt-Tab icon when `true`, or the
/// small (`SM_CXSMICON` / `SM_CYSMICON`) caption icon when `false`. The
/// system metrics are DPI-aware, so the loaded handle matches the size the
/// caption / Alt-Tab surface expects without blurry rescaling.
///
/// The handle is loaded with `LR_SHARED`, so the OS owns its lifetime and
/// it must never be destroyed by the caller.
///
/// Must be callable from any thread; the typical caller is the UI thread
/// that owns the window receiving the icon.
///
/// # Errors
///
/// Returns [`Error::Win32`] if `GetModuleHandleW` or `LoadImageW` fails —
/// for example when running from a binary that has no embedded id-1 icon
/// resource (such as a test host), in which case the caller should treat
/// the error as "no icon to set".
pub fn load_app_icon(large: bool) -> Result<HICON, Error> {
    let hinstance =
        unsafe { GetModuleHandleW(None) }.map_err(|e| Error::win32("GetModuleHandleW", e))?;
    let (width_metric, height_metric) = if large {
        (SM_CXICON, SM_CYICON)
    } else {
        (SM_CXSMICON, SM_CYSMICON)
    };
    let width = unsafe { GetSystemMetrics(width_metric) };
    let height = unsafe { GetSystemMetrics(height_metric) };
    // MAKEINTRESOURCE: a numeric resource id is passed as a PCWSTR whose
    // pointer value *is* the id (the high word must be zero).
    let resource = PCWSTR(APP_ICON_RESOURCE_ID as usize as *const u16);
    let handle = unsafe {
        LoadImageW(
            Some(hinstance.into()),
            resource,
            IMAGE_ICON,
            width,
            height,
            LR_DEFAULTSIZE | LR_SHARED,
        )
    }
    .map_err(|e| Error::win32("LoadImageW", e))?;
    Ok(HICON(handle.0))
}
