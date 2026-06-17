//! Win32 clipboard wrappers.
//!
//! Thread ownership: caller's UI thread (clipboard APIs are tied to the
//! window owner). All entry points open the clipboard, perform a single
//! operation, and close it before returning.

use std::sync::OnceLock;

use windows::core::w;
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::CF_UNICODETEXT;

use crate::Error;

/// Registered numeric id of the standard `"HTML Format"` clipboard
/// format, cached after the first lookup.
///
/// `RegisterClipboardFormatW` returns the same id for the same name for
/// the lifetime of the session, so this is computed once. A return value
/// of `0` means registration failed (out of format ids); callers treat
/// that as "HTML unavailable".
fn html_clipboard_format() -> u32 {
    static FORMAT: OnceLock<u32> = OnceLock::new();
    *FORMAT.get_or_init(|| unsafe { RegisterClipboardFormatW(w!("HTML Format")) })
}

/// `true` iff the clipboard currently advertises the `"HTML Format"`
/// payload (the format browsers / Office / most rich editors write).
#[must_use]
pub fn has_html() -> bool {
    let format = html_clipboard_format();
    if format == 0 {
        return false;
    }
    unsafe { IsClipboardFormatAvailable(format).is_ok() }
}

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

/// Read the `"HTML Format"` payload from the system clipboard and return
/// only the `StartFragment..EndFragment` slice as a `String`.
///
/// The CF_HTML format is a UTF-8 byte stream that begins with a small
/// ASCII header of `Key:Value` lines (`Version`, `StartHTML`, `EndHTML`,
/// `StartFragment`, `EndFragment`, optionally `SourceURL`), followed by
/// the HTML document. The `StartFragment`/`EndFragment` *byte* offsets
/// (decimal, measured from the start of the whole stream) bracket the
/// portion the source app actually copied — the only part worth
/// converting. We parse the header to extract that slice.
///
/// Returns `Ok(None)` when no HTML format is available, the header is
/// malformed, the offsets are out of range, or the fragment is empty
/// (the caller then falls back to plain text).
///
/// # Errors
///
/// Returns [`Error::Win32`] if `OpenClipboard` fails.
pub fn read_html(owner: HWND) -> Result<Option<String>, Error> {
    let format = html_clipboard_format();
    if format == 0 {
        return Ok(None);
    }
    unsafe {
        if IsClipboardFormatAvailable(format).is_err() {
            return Ok(None);
        }
        OpenClipboard(Some(owner)).map_err(|e| Error::win32("OpenClipboard", e))?;
        let result = (|| -> Result<Option<String>, Error> {
            let h = match GetClipboardData(format) {
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
            let ptr = GlobalLock(g_handle) as *const u8;
            if ptr.is_null() {
                return Ok(None);
            }
            // CF_HTML is a NUL-free UTF-8 stream, but some producers
            // append a trailing NUL terminator like CF_TEXT does. Copy
            // the raw bytes then trim a trailing NUL if present.
            let slice = std::slice::from_raw_parts(ptr, bytes);
            let owned = slice.to_vec();
            let _ = GlobalUnlock(g_handle);
            Ok(extract_html_fragment(&owned))
        })();
        let _ = CloseClipboard();
        result
    }
}

/// Parse a raw CF_HTML byte stream and return the
/// `StartFragment..EndFragment` slice as a UTF-8 `String`.
///
/// Pure (no Win32), so it is unit-testable. Returns `None` when the
/// header lacks valid fragment offsets, the offsets fall outside the
/// buffer, or the resulting fragment is empty. Falls back to the whole
/// document body (after the header) when the fragment markers are absent
/// but a body is present.
fn extract_html_fragment(raw: &[u8]) -> Option<String> {
    // Trim a single trailing NUL if a producer added one.
    let raw = match raw.last() {
        Some(0) => &raw[..raw.len() - 1],
        _ => raw,
    };
    // The header is ASCII; scan only the leading portion for the
    // offset keys. The header always precedes the first `<` of the
    // document, so bound the search to keep it cheap on huge payloads.
    let header_scan_len = raw.len().min(1024);
    let header = &raw[..header_scan_len];
    let start_fragment = parse_header_offset(header, b"StartFragment:");
    let end_fragment = parse_header_offset(header, b"EndFragment:");
    if let (Some(start), Some(end)) = (start_fragment, end_fragment) {
        if start <= end && end <= raw.len() {
            let fragment = &raw[start..end];
            let text = String::from_utf8_lossy(fragment).into_owned();
            if !text.trim().is_empty() {
                return Some(text);
            }
        }
    }
    // Fragment markers missing or unusable — fall back to the document
    // body after the header (everything from the first `<html` or `<`).
    let body_start = find_subslice(raw, b"<")?;
    let text = String::from_utf8_lossy(&raw[body_start..]).into_owned();
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Parse the decimal value following `key` (e.g. `b"StartFragment:"`) in
/// the CF_HTML ASCII header. Returns `None` when the key is absent or the
/// value is not a run of ASCII digits.
fn parse_header_offset(header: &[u8], key: &[u8]) -> Option<usize> {
    let key_at = find_subslice(header, key)?;
    let mut idx = key_at + key.len();
    // Skip optional leading whitespace.
    while idx < header.len() && (header[idx] == b' ' || header[idx] == b'\t') {
        idx += 1;
    }
    let digits_start = idx;
    while idx < header.len() && header[idx].is_ascii_digit() {
        idx += 1;
    }
    if idx == digits_start {
        return None;
    }
    let digits = std::str::from_utf8(&header[digits_start..idx]).ok()?;
    digits.parse::<usize>().ok()
}

/// Index of the first occurrence of `needle` in `haystack`, or `None`.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
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

#[cfg(test)]
mod html_fragment_tests {
    use super::{extract_html_fragment, find_subslice, parse_header_offset};

    /// Build a CF_HTML stream where the fragment markers point exactly at
    /// `fragment` inside a wrapper document, computing the byte offsets the
    /// way a real producer does.
    fn build_cf_html(fragment: &str) -> Vec<u8> {
        // Header with placeholder zero-padded offsets, then the body. We
        // assemble the body first so we can compute real offsets.
        let prefix = "<html><body><!--StartFragment-->";
        let suffix = "<!--EndFragment--></body></html>";
        let header_template = "Version:0.9\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\nStartFragment:0000000000\r\nEndFragment:0000000000\r\n";
        let header_len = header_template.len();
        let start_html = header_len;
        let start_fragment = header_len + prefix.len();
        let end_fragment = start_fragment + fragment.len();
        let end_html = end_fragment + suffix.len();
        let header = format!(
            "Version:0.9\r\nStartHTML:{start_html:010}\r\nEndHTML:{end_html:010}\r\nStartFragment:{start_fragment:010}\r\nEndFragment:{end_fragment:010}\r\n"
        );
        let mut out = String::new();
        out.push_str(&header);
        out.push_str(prefix);
        out.push_str(fragment);
        out.push_str(suffix);
        out.into_bytes()
    }

    #[test]
    fn extracts_exact_fragment() {
        let raw = build_cf_html("<b>hi</b>");
        assert_eq!(extract_html_fragment(&raw).as_deref(), Some("<b>hi</b>"));
    }

    #[test]
    fn tolerates_trailing_nul() {
        let mut raw = build_cf_html("<i>x</i>");
        raw.push(0);
        assert_eq!(extract_html_fragment(&raw).as_deref(), Some("<i>x</i>"));
    }

    #[test]
    fn falls_back_to_body_without_fragment_markers() {
        let raw = b"Version:0.9\r\nStartHTML:0\r\n<p>body</p>".to_vec();
        let out = extract_html_fragment(&raw).expect("fallback body");
        assert!(out.contains("<p>body</p>"));
    }

    #[test]
    fn whitespace_fragment_falls_back_to_body() {
        let raw = build_cf_html("   ");
        // The fragment is whitespace-only, so the fragment branch is
        // skipped; the body fallback returns the wrapper document, which
        // is non-empty. (The higher-level converter then trims it down.)
        let out = extract_html_fragment(&raw);
        assert!(out.is_some());
    }

    #[test]
    fn no_html_at_all_returns_none() {
        assert_eq!(extract_html_fragment(b"not html, no angle brackets"), None);
    }

    #[test]
    fn parse_header_offset_reads_decimal() {
        let header = b"StartFragment:0000000123\r\n";
        assert_eq!(parse_header_offset(header, b"StartFragment:"), Some(123));
    }

    #[test]
    fn parse_header_offset_missing_key() {
        assert_eq!(
            parse_header_offset(b"EndFragment:5", b"StartFragment:"),
            None
        );
    }

    #[test]
    fn find_subslice_basic() {
        assert_eq!(find_subslice(b"abcdef", b"cd"), Some(2));
        assert_eq!(find_subslice(b"abc", b"xyz"), None);
        assert_eq!(find_subslice(b"abc", b""), None);
    }
}
