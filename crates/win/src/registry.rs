//! Read-only Windows Registry helpers.
//!
//! The MSI installer records its install directory under
//! `HKLM\Software\Continuity\InstallDir`; the updater reads it to decide
//! whether the running executable is the installed copy. Nothing here
//! writes.
//!
//! Thread ownership: any thread; the registry API is thread-safe.

use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};

/// Read a `REG_SZ` value under `HKEY_LOCAL_MACHINE`. Returns `None` when
/// the key or value does not exist (or is not a string).
#[must_use]
pub fn read_hklm_string(subkey: &str, value: &str) -> Option<String> {
    let subkey_wide: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
    let value_wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let mut length: u32 = 0;
    let probe = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(subkey_wide.as_ptr()),
            PCWSTR(value_wide.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut length),
        )
    };
    if probe != ERROR_SUCCESS || length < 2 {
        return None;
    }
    let mut buffer = vec![0u16; (length as usize).div_ceil(2)];
    let read = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(subkey_wide.as_ptr()),
            PCWSTR(value_wide.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut length),
        )
    };
    if read != ERROR_SUCCESS {
        return None;
    }
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..end]))
}
