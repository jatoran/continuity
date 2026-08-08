//! UI Automation bridge shared with the desktop editor surface.

use continuity_host::{EditorIntent, SelectionIntent};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Accessibility::UiaRootObjectId;
use windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;

use super::state::EditorControlState;

impl EditorControlState {
    pub(super) fn publish_accessibility(&mut self) {
        let Ok(snapshot) = self.runtime.snapshot(self.buffer_id) else {
            return;
        };
        let change = self.surface.accessibility.publish_engine(
            &snapshot,
            unsafe { IsWindowEnabled(self.hwnd).as_bool() },
            self.surface.focus.has_keyboard_focus,
        );
        crate::window_accessibility::raise_accessibility_events(
            self.accessibility_provider.as_ref(),
            change,
        );
    }

    pub(super) fn handle_get_object(
        &mut self,
        hwnd: HWND,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Option<LRESULT> {
        if lparam.0 as i32 != UiaRootObjectId {
            return None;
        }
        self.accessibility_provider.get_or_insert_with(|| {
            crate::window_accessibility::create_native_editor_provider(
                hwnd,
                self.surface.accessibility.shared(),
            )
        });
        self.publish_accessibility();
        let provider = self
            .accessibility_provider
            .as_ref()
            .expect("invariant: child accessibility provider initialized");
        Some(crate::window_accessibility::return_raw_element_provider(
            hwnd, wparam, lparam, provider,
        ))
    }

    pub(super) fn handle_accessibility_selection(&mut self, lparam: LPARAM) -> LRESULT {
        let Ok(snapshot) = self.runtime.snapshot(self.buffer_id) else {
            return LRESULT(0);
        };
        let Some(selections) = crate::window_accessibility::selections_from_accessibility_request(
            &snapshot.rope,
            &snapshot.selections,
            lparam,
        ) else {
            return LRESULT(0);
        };
        if self
            .dispatch(EditorIntent::Select(SelectionIntent {
                buffer_id: self.buffer_id,
                selections,
            }))
            .is_err()
        {
            return LRESULT(0);
        }
        LRESULT(1)
    }
}
