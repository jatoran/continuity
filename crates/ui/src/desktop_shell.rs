//! Native desktop shell and its private Win32 message pump.
//!
//! The shell owns showing a top-level window, foreground activation, and the
//! `GetMessageW` loop. Editor-surface state does not own or run an application
//! event loop.

use windows::Win32::Graphics::Gdi::UpdateWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, DispatchMessageW, GetMessageW, PostQuitMessage, SetForegroundWindow,
    ShowWindow, TranslateMessage, MSG, SW_SHOW, SW_SHOWNOACTIVATE,
};

use crate::{Error, Window};

/// Desktop composition around one top-level Continuity window.
///
/// **Thread ownership:** constructed and consumed on the same UI thread as
/// its `Window`. The shell exclusively owns the thread's message pump.
pub struct DesktopShell {
    window: Box<Window>,
}

/// End the current desktop shell's private message pump.
///
/// Kept in the desktop composition so editor-surface state never owns
/// application-level termination behavior.
pub(crate) fn request_message_loop_quit() {
    unsafe { PostQuitMessage(0) };
}

impl DesktopShell {
    /// Compose a constructed top-level window into the native desktop shell.
    #[must_use]
    pub fn new(window: Box<Window>) -> Self {
        Self { window }
    }

    /// Show the window and run the shell-owned message pump.
    pub fn run(self) -> Result<(), Error> {
        self.run_inner(true)
    }

    /// Run the shell-owned message pump without showing or activating the
    /// window. Used by Win32 integration harnesses.
    pub fn run_hidden(self) -> Result<(), Error> {
        self.run_inner(false)
    }

    fn run_inner(self, show: bool) -> Result<(), Error> {
        let hwnd = self.window.hwnd();
        let activate = self.window.activate_on_show;
        // The WndProc re-enters through the pointer installed at WM_NCCREATE.
        // Reclaim the allocation after the shell receives WM_QUIT.
        let raw: *mut Window = Box::into_raw(self.window);
        unsafe {
            if show {
                if activate {
                    let _ = ShowWindow(hwnd, SW_SHOW);
                    let _ = BringWindowToTop(hwnd);
                    let _ = SetForegroundWindow(hwnd);
                } else {
                    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                }
                let _ = UpdateWindow(hwnd);
            }
            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            drop(Box::from_raw(raw));
        }
        Ok(())
    }
}
