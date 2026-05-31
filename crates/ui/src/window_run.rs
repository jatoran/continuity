//! `Window::run` and `Window::run_hidden` — the post-construction
//! message-pump entry points.
//!
//! Split from `window.rs` so the message-dispatch module stays
//! under the 600-line cap. Both methods take `self: Box<Self>` and
//! block until `WM_QUIT` arrives.

use windows::Win32::Graphics::Gdi::UpdateWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, DispatchMessageW, GetMessageW, SetForegroundWindow, ShowWindow,
    TranslateMessage, MSG, SW_SHOW,
};

use crate::window::Window;
use crate::Error;

impl Window {
    /// Show the window and run the message pump. Returns when the
    /// window is closed.
    pub fn run(self: Box<Self>) -> Result<(), Error> {
        self.run_inner(true)
    }

    /// Run the message pump **without** showing the window or
    /// bringing it to the foreground. Used by the §C1 Win32 e2e
    /// test harness so tests don't flash a visible window or steal
    /// focus. Identical to [`Self::run`] in every other respect.
    pub fn run_hidden(self: Box<Self>) -> Result<(), Error> {
        self.run_inner(false)
    }

    fn run_inner(self: Box<Self>, show: bool) -> Result<(), Error> {
        let hwnd = self.hwnd();
        // Leak the box for the lifetime of the message pump; we
        // rely on the self-pointer stored via WM_NCCREATE for
        // re-entry. We reclaim it at the end via `Box::from_raw`.
        let raw: *mut Window = Box::into_raw(self);
        unsafe {
            if show {
                let _ = ShowWindow(hwnd, SW_SHOW);
                // Cascaded new/tear-off windows must come up above the source.
                let _ = BringWindowToTop(hwnd);
                let _ = SetForegroundWindow(hwnd);
                let _ = UpdateWindow(hwnd);
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            // Clean up the boxed window after the pump exits.
            drop(Box::from_raw(raw));
        }
        Ok(())
    }
}
