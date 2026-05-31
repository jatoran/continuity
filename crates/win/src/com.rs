//! `ComGuard`: RAII wrapper around `CoInitializeEx`/`CoUninitialize`.

use windows::Win32::System::Com::{
    CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
};

use crate::Error;

/// RAII guard for COM initialization.
///
/// Calls `CoInitializeEx(COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE)`
/// on construction, and `CoUninitialize` on drop. Each thread that uses COM
/// (DirectWrite, virtual desktops, etc.) needs its own guard.
pub struct ComGuard {
    /// Marker so the type isn't `Send` or `Sync` — COM apartment is per-thread.
    _no_send: std::marker::PhantomData<*const ()>,
}

impl ComGuard {
    /// Initialize COM for the current thread.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Win32`] if `CoInitializeEx` fails. A previously
    /// initialized apartment that's compatible succeeds (S_FALSE).
    pub fn new() -> Result<Self, Error> {
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        if hr.is_err() {
            return Err(Error::win32(
                "CoInitializeEx",
                windows::core::Error::from_hresult(hr),
            ));
        }
        Ok(Self {
            _no_send: std::marker::PhantomData,
        })
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn com_guard_initializes_and_drops() {
        let _g = ComGuard::new().unwrap();
        // Drops at end of scope; CoUninitialize runs.
    }

    #[test]
    fn nested_guards_are_compatible() {
        // Same-apartment nested CoInitializeEx returns S_FALSE which is OK.
        let _outer = ComGuard::new().unwrap();
        let _inner = ComGuard::new().unwrap();
    }
}
