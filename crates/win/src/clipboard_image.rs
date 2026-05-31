//! Phase F5 — clipboard image readers (Win32 primitives).
//!
//! Two probes share the same open / lock / copy / close pattern as
//! [`crate::clipboard::read_text`]:
//!
//! * [`read_dib_bytes`] — `CF_DIB` (also accepts the V5 variant under
//!   the same numeric format id) → packed DIB bytes
//!   (`BITMAPINFOHEADER` + optional palette + pixel rows). The caller
//!   decodes; this primitive owns only the Win32 dance.
//! * [`read_dropped_image_paths`] — `CF_HDROP` → the list of file paths
//!   currently held on the clipboard (typically populated by Explorer
//!   when the user picks "Copy" on a selection of files). Note that
//!   `CF_HDROP` is not image-specific; the caller filters by extension.
//!
//! Thread ownership: caller's UI thread (clipboard APIs are tied to
//! the window owner).

use std::path::PathBuf;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::CF_HDROP;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

use crate::Error;

/// `CF_DIB` numeric id. The `windows` crate exposes it through
/// `CF_DIB` but only with the `Win32_System_Ole` feature; we mirror
/// the constant here to keep our feature set minimal.
const CF_DIB_FORMAT: u32 = 8;

/// `CF_DIBV5` numeric id. Newer Win10+ apps (Snip & Sketch, the
/// built-in screenshot key) write V5 DIBs. The byte layout starts
/// with a `BITMAPV5HEADER` whose first 40 bytes ARE a valid
/// `BITMAPINFOHEADER`, so a V5 blob can be decoded the same way as
/// a classic DIB after probing for the format id.
const CF_DIBV5_FORMAT: u32 = 17;

/// Read the raw DIB bytes off the clipboard. Returns `Ok(None)` when
/// neither `CF_DIB` nor `CF_DIBV5` is available.
///
/// The returned buffer is a copy of the global memory contents; the
/// caller owns it. Layout:
///
/// ```text
///   BITMAPINFOHEADER (40 bytes; V5 = 124 bytes, first 40 still valid)
///   [optional palette / BI_BITFIELDS masks]
///   pixel rows (DWORD-aligned)
/// ```
///
/// # Errors
///
/// Returns [`Error::Win32`] if `OpenClipboard` fails.
pub fn read_dib_bytes(owner: HWND) -> Result<Option<Vec<u8>>, Error> {
    unsafe {
        let format = if IsClipboardFormatAvailable(CF_DIBV5_FORMAT).is_ok() {
            CF_DIBV5_FORMAT
        } else if IsClipboardFormatAvailable(CF_DIB_FORMAT).is_ok() {
            CF_DIB_FORMAT
        } else {
            return Ok(None);
        };
        OpenClipboard(Some(owner)).map_err(|e| Error::win32("OpenClipboard", e))?;
        let result = (|| -> Result<Option<Vec<u8>>, Error> {
            let handle = match GetClipboardData(format) {
                Ok(h) => h,
                Err(_) => return Ok(None),
            };
            if handle.0.is_null() {
                return Ok(None);
            }
            let g_handle = windows::Win32::Foundation::HGLOBAL(handle.0);
            let bytes = GlobalSize(g_handle);
            if bytes == 0 {
                return Ok(None);
            }
            let ptr = GlobalLock(g_handle) as *const u8;
            if ptr.is_null() {
                return Ok(None);
            }
            let slice = std::slice::from_raw_parts(ptr, bytes);
            let copy = slice.to_vec();
            let _ = GlobalUnlock(g_handle);
            Ok(Some(copy))
        })();
        let _ = CloseClipboard();
        result
    }
}

/// Read the `CF_HDROP` file list off the clipboard. Returns an empty
/// vector when the format is unavailable. The caller filters paths by
/// extension to decide whether they belong on the image-import path.
///
/// # Errors
///
/// Returns [`Error::Win32`] if `OpenClipboard` fails.
pub fn read_dropped_image_paths(owner: HWND) -> Result<Vec<PathBuf>, Error> {
    unsafe {
        if IsClipboardFormatAvailable(CF_HDROP.0.into()).is_err() {
            return Ok(Vec::new());
        }
        OpenClipboard(Some(owner)).map_err(|e| Error::win32("OpenClipboard", e))?;
        let result = (|| -> Result<Vec<PathBuf>, Error> {
            let handle = match GetClipboardData(CF_HDROP.0.into()) {
                Ok(h) => h,
                Err(_) => return Ok(Vec::new()),
            };
            if handle.0.is_null() {
                return Ok(Vec::new());
            }
            let hdrop = HDROP(handle.0);
            let count = DragQueryFileW(hdrop, u32::MAX, None);
            let mut paths = Vec::with_capacity(count as usize);
            for idx in 0..count {
                let len = DragQueryFileW(hdrop, idx, None);
                if len == 0 {
                    continue;
                }
                let mut buf = vec![0u16; len as usize + 1];
                let written = DragQueryFileW(hdrop, idx, Some(&mut buf));
                if written == 0 {
                    continue;
                }
                paths.push(PathBuf::from(String::from_utf16_lossy(
                    &buf[..written as usize],
                )));
            }
            Ok(paths)
        })();
        let _ = CloseClipboard();
        result
    }
}
