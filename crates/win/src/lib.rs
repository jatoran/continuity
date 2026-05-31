#![warn(missing_docs)]
//! Win32 wrappers: COM init RAII, DPI awareness, window class registration,
//! and hidden-window creation.
//!
//! Foundation layer for everything that touches the OS.

pub mod clipboard;
pub mod clipboard_image;
pub mod com;
pub mod dpi;
pub mod error;
pub mod ime;
pub mod virtual_desktop;
pub mod window;

pub use com::ComGuard;
pub use dpi::{dpi_for_window, set_per_monitor_dpi_v2};
pub use error::Error;
pub use virtual_desktop::VirtualDesktopManager;
pub use window::{HiddenWindow, WindowClass};
/// Re-export of the opaque HIMC handle so callers don't reach into the
/// `windows` crate path.
pub use windows::Win32::UI::Input::Ime::HIMC;
