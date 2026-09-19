//! Minimal synchronous HTTPS GET over WinHTTP.
//!
//! The workspace forbids `reqwest`/`tokio`; the update checker needs one
//! blocking GET (the GitHub Releases API, then the release assets), so
//! this wraps the system WinHTTP stack directly. WinHTTP honours the
//! machine proxy configuration, validates TLS against the Windows
//! certificate store, and follows same-scheme redirects (GitHub asset
//! downloads bounce to `objects.githubusercontent.com`).
//!
//! Thread ownership: call from any background thread; never from the UI
//! thread (a GET blocks for the round trip).

use core::ffi::c_void;

use windows::core::PCWSTR;
use windows::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders,
    WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_OPEN_REQUEST_FLAGS,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
};

use crate::error::Error;

/// Largest body this helper will buffer (64 MiB) — well above any release
/// asset, low enough that a misbehaving server cannot exhaust memory.
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

/// Parsed `https://host[:port]/path?query` URL.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpsUrl {
    /// Host name without scheme or port.
    pub host: String,
    /// TCP port (443 unless the URL names one).
    pub port: u16,
    /// Path plus query, always starting with `/`.
    pub object: String,
}

impl HttpsUrl {
    /// Split an `https://` URL into host, port, and object path.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidUrl`] for anything that is not an absolute
    /// `https://` URL with a host.
    pub fn parse(url: &str) -> Result<Self, Error> {
        let rest = url
            .strip_prefix("https://")
            .ok_or_else(|| Error::InvalidUrl(url.to_string()))?;
        let (authority, object) = match rest.find('/') {
            Some(index) => (&rest[..index], &rest[index..]),
            None => (rest, "/"),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty() => {
                (host, port.parse::<u16>().unwrap_or(443))
            }
            _ => (authority, 443),
        };
        if host.is_empty() {
            return Err(Error::InvalidUrl(url.to_string()));
        }
        Ok(Self {
            host: host.to_string(),
            port,
            object: object.to_string(),
        })
    }
}

/// Owned WinHTTP handle closed on drop.
struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Perform a blocking HTTPS GET and return the response body.
///
/// `user_agent` is sent as the `User-Agent` header (GitHub's API rejects
/// requests without one).
///
/// # Errors
///
/// Returns [`Error::InvalidUrl`] for a non-`https://` URL,
/// [`Error::HttpStatus`] for any non-2xx status, [`Error::HttpTooLarge`]
/// when the body exceeds the buffer cap, and [`Error::Win32`] when a
/// WinHTTP call fails (no network, TLS failure, proxy refusal).
pub fn https_get(url: &str, user_agent: &str) -> Result<Vec<u8>, Error> {
    let parsed = HttpsUrl::parse(url)?;
    let agent = wide(user_agent);
    let session = Handle(unsafe {
        WinHttpOpen(
            PCWSTR(agent.as_ptr()),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        )
    });
    if session.0.is_null() {
        return Err(Error::win32(
            "WinHttpOpen",
            windows::core::Error::from_win32(),
        ));
    }
    let host = wide(&parsed.host);
    let connection =
        Handle(unsafe { WinHttpConnect(session.0, PCWSTR(host.as_ptr()), parsed.port, 0) });
    if connection.0.is_null() {
        return Err(Error::win32(
            "WinHttpConnect",
            windows::core::Error::from_win32(),
        ));
    }
    let verb = wide("GET");
    let object = wide(&parsed.object);
    let request = Handle(unsafe {
        WinHttpOpenRequest(
            connection.0,
            PCWSTR(verb.as_ptr()),
            PCWSTR(object.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_OPEN_REQUEST_FLAGS(WINHTTP_FLAG_SECURE.0),
        )
    });
    if request.0.is_null() {
        return Err(Error::win32(
            "WinHttpOpenRequest",
            windows::core::Error::from_win32(),
        ));
    }
    let headers = wide("Accept: application/vnd.github+json, */*\r\n");
    unsafe {
        WinHttpSendRequest(
            request.0,
            Some(&headers[..headers.len() - 1]),
            None,
            0,
            0,
            0,
        )
        .map_err(|e| Error::win32("WinHttpSendRequest", e))?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut())
            .map_err(|e| Error::win32("WinHttpReceiveResponse", e))?;
    }
    let status = query_status_code(request.0)?;
    if !(200..300).contains(&status) {
        return Err(Error::HttpStatus {
            status,
            url: url.to_string(),
        });
    }
    read_body(request.0)
}

fn query_status_code(request: *mut c_void) -> Result<u32, Error> {
    let mut status: u32 = 0;
    let mut length = std::mem::size_of::<u32>() as u32;
    unsafe {
        WinHttpQueryHeaders(
            request,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(std::ptr::addr_of_mut!(status).cast::<c_void>()),
            &mut length,
            std::ptr::null_mut(),
        )
        .map_err(|e| Error::win32("WinHttpQueryHeaders", e))?;
    }
    Ok(status)
}

fn read_body(request: *mut c_void) -> Result<Vec<u8>, Error> {
    let mut body = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let mut read: u32 = 0;
        unsafe {
            WinHttpReadData(
                request,
                chunk.as_mut_ptr().cast::<c_void>(),
                chunk.len() as u32,
                &mut read,
            )
            .map_err(|e| Error::win32("WinHttpReadData", e))?;
        }
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read as usize]);
        if body.len() > MAX_BODY_BYTES {
            return Err(Error::HttpTooLarge(MAX_BODY_BYTES));
        }
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::HttpsUrl;

    #[test]
    fn parses_host_port_and_object() {
        let url =
            HttpsUrl::parse("https://api.github.com/repos/jatoran/continuity/releases/latest")
                .expect("parse");
        assert_eq!(url.host, "api.github.com");
        assert_eq!(url.port, 443);
        assert_eq!(url.object, "/repos/jatoran/continuity/releases/latest");
        let with_port = HttpsUrl::parse("https://example.com:8443").expect("parse");
        assert_eq!((with_port.port, with_port.object.as_str()), (8443, "/"));
    }

    #[test]
    fn rejects_non_https_and_empty_host() {
        assert!(HttpsUrl::parse("http://example.com/").is_err());
        assert!(HttpsUrl::parse("https:///path").is_err());
    }
}
