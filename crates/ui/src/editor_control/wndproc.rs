//! Win32 child-class procedure and lifecycle routing.

use continuity_host::{EditorIntent, HostRequest};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Shell::{DragFinish, DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GetWindowLongPtrW, SetWindowLongPtrW, CREATESTRUCTW, DLGC_WANTARROWS,
    DLGC_WANTCHARS, DLGC_WANTTAB, GWLP_USERDATA, WM_CAPTURECHANGED, WM_CHAR, WM_CONTEXTMENU,
    WM_DESTROY, WM_DPICHANGED_AFTERPARENT, WM_ENABLE, WM_ERASEBKGND, WM_GETDLGCODE, WM_GETOBJECT,
    WM_IME_COMPOSITION, WM_IME_ENDCOMPOSITION, WM_IME_STARTCOMPOSITION, WM_KEYDOWN, WM_KILLFOCUS,
    WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCCREATE,
    WM_NCDESTROY, WM_PAINT, WM_SETFOCUS, WM_SIZE,
};

use super::state::EditorControlState;
use super::TabBehavior;

/// Window procedure registered for every embeddable child class.
pub(super) unsafe extern "system" fn editor_control_wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let dispatch = std::panic::AssertUnwindSafe(|| {
        if message == WM_NCCREATE {
            let create = lparam.0 as *const CREATESTRUCTW;
            if !create.is_null() {
                let state = unsafe { (*create).lpCreateParams } as isize;
                unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, state) };
            }
        }
        let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut EditorControlState;
        if state.is_null() {
            return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
        }
        let state = unsafe { &mut *state };
        match state.handle_message(hwnd, message, wparam, lparam) {
            Ok(Some(result)) => result,
            Ok(None) => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
            Err(error) => {
                eprintln!("continuity-ui: editor control message {message} failed: {error}");
                LRESULT(0)
            }
        }
    });
    std::panic::catch_unwind(dispatch).unwrap_or_else(|_| {
        eprintln!("continuity-ui: editor control panic in message {message}");
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    })
}

impl EditorControlState {
    fn handle_message(
        &mut self,
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Result<Option<LRESULT>, crate::Error> {
        let result = match message {
            message if message == crate::window_accessibility::ACCESSIBILITY_SELECTION_MESSAGE => {
                Some(self.handle_accessibility_selection(lparam))
            }
            WM_GETOBJECT => self.handle_get_object(hwnd, wparam, lparam),
            WM_PAINT => {
                self.paint(hwnd)?;
                Some(LRESULT(0))
            }
            WM_ERASEBKGND => Some(LRESULT(1)),
            WM_SIZE => {
                if self.is_live {
                    self.resize_render_resources(hwnd)?;
                    self.dispatch(EditorIntent::ViewportChanged(self.viewport()))?;
                }
                Some(LRESULT(0))
            }
            WM_DPICHANGED_AFTERPARENT => {
                self.rebind_dpi(hwnd)?;
                self.dispatch(EditorIntent::ViewportChanged(self.viewport()))?;
                self.invalidate();
                Some(LRESULT(0))
            }
            WM_CHAR => {
                self.handle_char(wparam.0 as u16)?;
                Some(LRESULT(0))
            }
            WM_KEYDOWN => self.handle_key_down(wparam.0 as u16)?.then_some(LRESULT(0)),
            WM_GETDLGCODE => {
                let mut code = DLGC_WANTARROWS | DLGC_WANTCHARS;
                if self.options.tab_behavior == TabBehavior::InsertIndent {
                    code |= DLGC_WANTTAB;
                }
                Some(LRESULT(code as isize))
            }
            WM_SETFOCUS => {
                self.handle_focus(true)?;
                Some(LRESULT(0))
            }
            WM_KILLFOCUS => {
                self.handle_focus(false)?;
                Some(LRESULT(0))
            }
            WM_MOUSEWHEEL => {
                self.handle_wheel(wparam)?;
                Some(LRESULT(0))
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_LBUTTONUP | WM_MOUSEMOVE | WM_MOUSELEAVE => {
                self.handle_pointer(message, wparam, lparam)?;
                Some(LRESULT(0))
            }
            WM_CAPTURECHANGED => {
                self.drag_anchor = None;
                Some(LRESULT(0))
            }
            WM_CONTEXTMENU => {
                let (x_dip, y_dip) = context_menu_point(hwnd, lparam, self.scale());
                self.dispatch(EditorIntent::Request(HostRequest::ContextMenu {
                    x_dip,
                    y_dip,
                }))?;
                Some(LRESULT(0))
            }
            windows::Win32::UI::WindowsAndMessaging::WM_DROPFILES => {
                self.handle_drop_files(wparam)?;
                Some(LRESULT(0))
            }
            WM_IME_STARTCOMPOSITION => {
                self.handle_ime_start()?;
                Some(LRESULT(0))
            }
            WM_IME_COMPOSITION => {
                self.handle_ime_composition(hwnd, lparam)?;
                Some(LRESULT(0))
            }
            WM_IME_ENDCOMPOSITION => {
                self.handle_ime_end()?;
                Some(LRESULT(0))
            }
            WM_ENABLE => {
                self.publish_accessibility();
                self.invalidate();
                Some(LRESULT(0))
            }
            WM_DESTROY => Some(LRESULT(0)),
            WM_NCDESTROY => {
                self.is_live = false;
                self.surface.render.renderer = None;
                self.accessibility_provider = None;
                unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
                None
            }
            _ => None,
        };
        Ok(result)
    }

    fn handle_drop_files(&mut self, wparam: WPARAM) -> Result<(), crate::Error> {
        let drop = HDROP(wparam.0 as *mut core::ffi::c_void);
        let count = unsafe { DragQueryFileW(drop, u32::MAX, None) };
        let mut paths = Vec::with_capacity(count as usize);
        for index in 0..count {
            let length = unsafe { DragQueryFileW(drop, index, None) };
            if length == 0 {
                continue;
            }
            let mut buffer = vec![0u16; length as usize + 1];
            let written = unsafe { DragQueryFileW(drop, index, Some(&mut buffer)) };
            if written > 0 {
                paths.push(String::from_utf16_lossy(&buffer[..written as usize]));
            }
        }
        unsafe { DragFinish(drop) };
        self.dispatch(EditorIntent::Request(HostRequest::DroppedFiles(paths)))
    }
}

fn context_menu_point(hwnd: HWND, lparam: LPARAM, scale: f32) -> (f32, f32) {
    if lparam.0 == -1 {
        return (0.0, 0.0);
    }
    let mut point = POINT {
        x: (lparam.0 as i16) as i32,
        y: ((lparam.0 >> 16) as i16) as i32,
    };
    unsafe {
        let _ = ScreenToClient(hwnd, &mut point);
    }
    (point.x as f32 / scale, point.y as f32 / scale)
}
