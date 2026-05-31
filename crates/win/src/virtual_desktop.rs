//! Thin wrapper around `IVirtualDesktopManager` (Phase 14).
//!
//! Provides only the documented, stable surface: query a window's desktop
//! GUID, and move a window to a known GUID. The undocumented
//! `IVirtualDesktopManagerInternal` (which can list desktops, create new
//! ones, switch the active desktop) is intentionally out of scope — the
//! editor never auto-switches the user between desktops, per spec §6.
//!
//! **Thread ownership**: each UI thread that uses this type must already
//! hold a [`crate::ComGuard`]. The wrapper itself is `!Send` to enforce
//! this — `IVirtualDesktopManager` is a per-apartment object.

use windows::core::GUID;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::IVirtualDesktopManager;

use crate::Error;

/// CLSID of the `VirtualDesktopManager` coclass — documented on MSDN.
const CLSID_VIRTUAL_DESKTOP_MANAGER: GUID = GUID::from_u128(0xaa509086_5ca9_4c25_8f95_589d3c07b48a);

/// Idiomatic wrapper around `IVirtualDesktopManager`.
pub struct VirtualDesktopManager {
    inner: IVirtualDesktopManager,
    /// Marker so the type isn't `Send`. COM apartment is per-thread.
    _no_send: std::marker::PhantomData<*const ()>,
}

impl VirtualDesktopManager {
    /// `CoCreateInstance` the manager. Caller must already have COM
    /// initialized on the current thread.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Win32`] if `CoCreateInstance` fails (typically only
    /// on pre-Win10 hosts).
    pub fn new() -> Result<Self, Error> {
        let inner: IVirtualDesktopManager =
            unsafe { CoCreateInstance(&CLSID_VIRTUAL_DESKTOP_MANAGER, None, CLSCTX_INPROC_SERVER) }
                .map_err(|e| Error::win32("CoCreateInstance(VirtualDesktopManager)", e))?;
        Ok(Self {
            inner,
            _no_send: std::marker::PhantomData,
        })
    }

    /// Return the GUID of the desktop currently hosting `hwnd`, or `None`
    /// when the call fails (transient failure / shell not yet ready).
    #[must_use]
    pub fn desktop_id_of_window(&self, hwnd: HWND) -> Option<[u8; 16]> {
        unsafe { self.inner.GetWindowDesktopId(hwnd) }
            .ok()
            .map(guid_to_bytes)
    }

    /// Move `hwnd` to the desktop identified by `guid_bytes`. Returns
    /// `Ok(true)` when the call succeeds; the manager rejects unknown
    /// GUIDs with `E_INVALIDARG`, in which case we return `Ok(false)` so
    /// the caller can fall back to the active desktop without surfacing
    /// the error.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Win32`] only on unexpected failures.
    pub fn move_window_to_desktop(&self, hwnd: HWND, guid_bytes: [u8; 16]) -> Result<bool, Error> {
        let guid = bytes_to_guid(guid_bytes);
        match unsafe { self.inner.MoveWindowToDesktop(hwnd, &guid) } {
            Ok(()) => Ok(true),
            Err(e) if e.code() == windows::core::HRESULT(-2147024809) => {
                // E_INVALIDARG — desktop GUID is no longer present. Fall
                // back to the active desktop silently per spec §6.
                Ok(false)
            }
            Err(e) => Err(Error::win32("MoveWindowToDesktop", e)),
        }
    }

    /// Borrow the underlying COM pointer for advanced callers.
    #[must_use]
    pub fn raw(&self) -> &IVirtualDesktopManager {
        &self.inner
    }
}

fn guid_to_bytes(g: GUID) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&g.data1.to_be_bytes());
    out[4..6].copy_from_slice(&g.data2.to_be_bytes());
    out[6..8].copy_from_slice(&g.data3.to_be_bytes());
    out[8..16].copy_from_slice(&g.data4);
    out
}

fn bytes_to_guid(b: [u8; 16]) -> GUID {
    let data1 = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    let data2 = u16::from_be_bytes([b[4], b[5]]);
    let data3 = u16::from_be_bytes([b[6], b[7]]);
    let mut data4 = [0u8; 8];
    data4.copy_from_slice(&b[8..16]);
    GUID {
        data1,
        data2,
        data3,
        data4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_round_trips_through_bytes() {
        let g = GUID::from_u128(0xdead_beef_cafe_babe_1234_5678_9abc_def0);
        let b = guid_to_bytes(g);
        let g2 = bytes_to_guid(b);
        assert_eq!(g.data1, g2.data1);
        assert_eq!(g.data2, g2.data2);
        assert_eq!(g.data3, g2.data3);
        assert_eq!(g.data4, g2.data4);
    }
}
