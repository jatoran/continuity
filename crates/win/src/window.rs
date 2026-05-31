//! Hidden top-level window plumbing for offscreen / smoke-test rendering.

use std::sync::atomic::{AtomicU32, Ordering};

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, UnregisterClassW,
    CW_USEDEFAULT, HCURSOR, HICON, HMENU, WINDOW_EX_STYLE, WNDCLASSW, WNDPROC, WS_OVERLAPPEDWINDOW,
};

use crate::Error;

static CLASS_NAMES: AtomicU32 = AtomicU32::new(0);

/// RAII registration of a Win32 window class. The class is registered on
/// construction and unregistered on drop.
pub struct WindowClass {
    name: HSTRING,
    hinstance: HMODULE,
}

impl WindowClass {
    /// Register a fresh window class with the default passthrough wndproc.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Win32`] if `RegisterClassW` fails.
    pub(crate) fn register_unique(prefix: &str) -> Result<Self, Error> {
        Self::register_unique_with_proc(prefix, Some(default_wndproc))
    }

    /// Register a fresh window class with a caller-supplied wndproc. Each
    /// call gets a unique class name so concurrent tests / windows don't
    /// collide.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Win32`] if `RegisterClassW` fails.
    pub fn register_unique_with_proc(prefix: &str, wndproc: WNDPROC) -> Result<Self, Error> {
        let n = CLASS_NAMES.fetch_add(1, Ordering::Relaxed);
        let name = HSTRING::from(format!("{prefix}_{n}_{:p}", &CLASS_NAMES));
        let hinstance =
            unsafe { GetModuleHandleW(None) }.map_err(|e| Error::win32("GetModuleHandleW", e))?;

        let class = WNDCLASSW {
            style: Default::default(),
            lpfnWndProc: wndproc,
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance.into(),
            hIcon: HICON::default(),
            hCursor: HCURSOR::default(),
            hbrBackground: Default::default(),
            lpszMenuName: PCWSTR::null(),
            lpszClassName: PCWSTR(name.as_ptr()),
        };
        let atom = unsafe { RegisterClassW(&class) };
        if atom == 0 {
            return Err(Error::win32(
                "RegisterClassW",
                windows::core::Error::from_win32(),
            ));
        }
        Ok(Self { name, hinstance })
    }

    /// The class name (for `CreateWindowExW`).
    #[must_use]
    pub fn name(&self) -> &HSTRING {
        &self.name
    }

    /// The module handle the class was registered against.
    #[must_use]
    pub fn hinstance(&self) -> HMODULE {
        self.hinstance
    }
}

impl Drop for WindowClass {
    fn drop(&mut self) {
        unsafe {
            let _ = UnregisterClassW(PCWSTR(self.name.as_ptr()), Some(self.hinstance.into()));
        }
    }
}

/// A hidden top-level window for offscreen rendering smoke tests.
///
/// Registers a unique window class and creates an unshown `WS_OVERLAPPEDWINDOW`
/// HWND at default position. Destroys the HWND on drop; the class is held
/// alive by `WindowClass` for the window's lifetime.
pub struct HiddenWindow {
    hwnd: HWND,
    _class: WindowClass,
}

impl HiddenWindow {
    /// Create a hidden window of size `(width, height)` device pixels.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Win32`] if class registration or window creation fails.
    pub fn create(width: i32, height: i32) -> Result<Self, Error> {
        let class = WindowClass::register_unique("ContinuityHidden")?;
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                PCWSTR(class.name().as_ptr()),
                &HSTRING::from("continuity-hidden"),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                width,
                height,
                None,
                Option::<HMENU>::None,
                Some(class.hinstance().into()),
                None,
            )
        }
        .map_err(|e| Error::win32("CreateWindowExW", e))?;
        Ok(Self {
            hwnd,
            _class: class,
        })
    }

    /// The HWND.
    #[must_use]
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }
}

impl Drop for HiddenWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

unsafe extern "system" fn default_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_register_and_unregister() {
        let _c = WindowClass::register_unique("ContinuityClassTest").unwrap();
    }

    #[test]
    fn hidden_window_create_and_destroy() {
        // Per-monitor DPI must be opted-in before creating the HWND, but the
        // test process may already be DPI-aware via another test; tolerate
        // either case.
        let _ = crate::set_per_monitor_dpi_v2();
        let w = HiddenWindow::create(64, 64).unwrap();
        assert!(!w.hwnd().is_invalid());
    }
}
