//! Win32 clipboard wrappers.
//!
//! Thread ownership: caller's UI thread (clipboard APIs are tied to the
//! window owner). All entry points open the clipboard, perform a single
//! operation, and close it before returning.

use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::CF_UNICODETEXT;

use crate::Error;

/// Read `CF_UNICODETEXT` from the system clipboard.
///
/// Returns `Ok(None)` when no Unicode text is available (the only format
/// the editor consumes — paste-as-plain semantics drop everything else).
///
/// # Errors
///
/// Returns [`Error::Win32`] if `OpenClipboard` fails.
pub fn read_text(owner: HWND) -> Result<Option<String>, Error> {
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT.0.into()).is_err() {
            return Ok(None);
        }
        OpenClipboard(Some(owner)).map_err(|e| Error::win32("OpenClipboard", e))?;
        let result = (|| -> Result<Option<String>, Error> {
            let h = match GetClipboardData(CF_UNICODETEXT.0.into()) {
                Ok(h) => h,
                Err(_) => return Ok(None),
            };
            if h.0.is_null() {
                return Ok(None);
            }
            let g_handle = windows::Win32::Foundation::HGLOBAL(h.0);
            let bytes = GlobalSize(g_handle);
            if bytes == 0 {
                return Ok(None);
            }
            let ptr = GlobalLock(g_handle) as *const u16;
            if ptr.is_null() {
                return Ok(None);
            }
            // Bytes may include a trailing NUL (typical) but is not required.
            let max_units = bytes / std::mem::size_of::<u16>();
            let slice = std::slice::from_raw_parts(ptr, max_units);
            let len_no_nul = slice.iter().position(|&u| u == 0).unwrap_or(slice.len());
            let s = String::from_utf16_lossy(&slice[..len_no_nul]);
            let _ = GlobalUnlock(g_handle);
            Ok(Some(s))
        })();
        let _ = CloseClipboard();
        result
    }
}

/// Write `text` to the system clipboard as `CF_UNICODETEXT`.
///
/// Empties any prior clipboard contents on success.
///
/// # Errors
///
/// Returns [`Error::Win32`] if any of the open / alloc / set steps fail.
pub fn write_text(owner: HWND, text: &str) -> Result<(), Error> {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    let bytes = wide.len() * std::mem::size_of::<u16>();
    unsafe {
        OpenClipboard(Some(owner)).map_err(|e| Error::win32("OpenClipboard", e))?;
        let result = (|| -> Result<(), Error> {
            EmptyClipboard().map_err(|e| Error::win32("EmptyClipboard", e))?;
            let h =
                GlobalAlloc(GMEM_MOVEABLE, bytes).map_err(|e| Error::win32("GlobalAlloc", e))?;
            let dst = GlobalLock(h) as *mut u16;
            if dst.is_null() {
                return Err(Error::win32(
                    "GlobalLock",
                    windows::core::Error::from_win32(),
                ));
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
            let _ = GlobalUnlock(h);
            // After SetClipboardData the system owns the handle. On failure
            // we do not free it; GlobalAlloc(GMEM_MOVEABLE) is reaped on
            // process exit.
            SetClipboardData(CF_UNICODETEXT.0.into(), Some(HANDLE(h.0)))
                .map_err(|e| Error::win32("SetClipboardData", e))?;
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}

/// `true` iff the clipboard currently holds `CF_UNICODETEXT`.
pub fn has_text() -> bool {
    unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT.0.into()).is_ok() }
}
