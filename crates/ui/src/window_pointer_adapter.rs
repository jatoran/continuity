//! Native behavior adapter for normalized editor-surface pointer intents.

use continuity_host::{PointerButton, PointerIntent, PointerPhase};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_MENU;
use windows::Win32::UI::WindowsAndMessaging::{
    WM_CAPTURECHANGED, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MOUSEMOVE,
};

use crate::editor_surface::pointer::PointerAction;
use crate::window_helpers::lparam_to_xy;
use crate::window_input_modifiers::is_key_down;
use crate::Window;

impl Window {
    /// Decode and route the Win32 pointer messages owned by the editor
    /// surface. Returns `None` for non-pointer messages.
    pub(crate) fn handle_native_pointer_message(
        &mut self,
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Option<LRESULT> {
        let changed = match message {
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_LBUTTONUP | WM_MBUTTONDOWN => {
                let (x, y) = lparam_to_xy(lparam);
                let (x, y) = self.physical_point_to_dip(x, y);
                let (phase, button, click_count) = match message {
                    WM_LBUTTONDOWN => (PointerPhase::Down, PointerButton::Primary, 1),
                    WM_LBUTTONDBLCLK => (PointerPhase::Down, PointerButton::Primary, 2),
                    WM_LBUTTONUP => (PointerPhase::Up, PointerButton::Primary, 1),
                    _ => (PointerPhase::Down, PointerButton::Middle, 1),
                };
                self.dispatch_native_pointer(x, y, phase, button, click_count, wparam.0 as u32)
            }
            WM_MOUSEMOVE => {
                let (x, y) = lparam_to_xy(lparam);
                let (x, y) = self.physical_point_to_dip(x, y);
                self.ensure_mouse_leave_tracking(hwnd);
                let button = if wparam.0 as u32 & 0x0001 != 0 {
                    PointerButton::Primary
                } else if wparam.0 as u32 & 0x0010 != 0 {
                    PointerButton::Middle
                } else if wparam.0 as u32 & 0x0002 != 0 {
                    PointerButton::Secondary
                } else {
                    PointerButton::None
                };
                self.dispatch_native_pointer(x, y, PointerPhase::Move, button, 0, wparam.0 as u32)
            }
            WM_MOUSELEAVE => {
                self.dispatch_native_pointer(0, 0, PointerPhase::Leave, PointerButton::None, 0, 0)
            }
            WM_CAPTURECHANGED => {
                let _ = self.dispatch_native_pointer(
                    0,
                    0,
                    PointerPhase::Cancel,
                    PointerButton::None,
                    0,
                    0,
                );
                return Some(LRESULT(0));
            }
            _ => return None,
        };
        if changed {
            self.invalidate(hwnd);
        }
        Some(LRESULT(0))
    }

    /// Translate Win32 coordinates/button bits into the shared host contract.
    pub(crate) fn dispatch_native_pointer(
        &mut self,
        x: i32,
        y: i32,
        phase: PointerPhase,
        button: PointerButton,
        click_count: u8,
        key_state: u32,
    ) -> bool {
        self.dispatch_pointer_intent(PointerIntent {
            x_dip: x as f32,
            y_dip: y as f32,
            button,
            phase,
            click_count,
            is_primary_down: key_state & 0x0001 != 0,
            is_secondary_down: key_state & 0x0002 != 0,
            is_middle_down: key_state & 0x0010 != 0,
            is_shift_down: key_state & 0x0004 != 0,
            is_control_down: key_state & 0x0008 != 0,
            is_alt_down: is_key_down(VK_MENU.0),
        })
    }

    /// Route a platform-neutral pointer intent through the surface controller,
    /// then invoke the existing native hit-test/drag behavior.
    pub(crate) fn dispatch_pointer_intent(&mut self, intent: PointerIntent) -> bool {
        match self.surface.pointer.route_intent(intent) {
            PointerAction::PrimaryDown { x, y, key_state } => {
                self.on_left_button_down(x, y, key_state)
            }
            PointerAction::PrimaryDoubleDown { x, y } => self.on_left_button_dbl(x, y),
            PointerAction::PrimaryUp { x, y } => {
                let changed = self.on_left_button_up(x, y);
                self.mouse_state.dragging = false;
                self.mouse_state.tab_drag = None;
                self.mouse_state.splitter_drag = None;
                changed
            }
            PointerAction::MiddleDown { x, y } => self.on_middle_button_down(x, y),
            PointerAction::Move { x, y, key_state } => self.on_mouse_move(x, y, key_state),
            PointerAction::Leave => self.on_mouse_leave(),
            PointerAction::Cancel => {
                self.on_capture_changed();
                false
            }
            PointerAction::Ignored => false,
        }
    }
}
