//! Reveal an already-open file buffer in this window.
//!
//! When the user reopens a file that already has a live buffer, the app
//! registry routes a [`crate::WindowControl::RevealBufferTab`] to the
//! window that owns (or last owned) the buffer instead of spawning a
//! duplicate top-level window. This module activates the buffer's tab
//! (adopting a fresh tab when the old one was closed but the buffer is
//! still alive), brings the window to the foreground, and reconciles the
//! buffer against the freshly-read disk bytes via
//! [`Window::reconcile_file_buffer`].
//!
//! Thread ownership: UI thread of one window.

use continuity_buffer::{BufferId, FileAssociation};
use windows::Win32::UI::WindowsAndMessaging::{
    IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE,
};

use crate::pane_tree::{PaneId, TabId};
use crate::window::Window;
use crate::window_file::FileBanner;
use crate::window_helpers::invalidate_hwnd_with_reason;

impl Window {
    /// Surface `buffer_id` in this window and reconcile it against the
    /// freshly-read disk bytes. `notices` are launch-time banners (e.g. an
    /// encoding notice from a forwarded open) shown after reconciliation.
    /// No-op when the buffer is not alive.
    pub(crate) fn reveal_file_buffer_tab(
        &mut self,
        buffer_id: BufferId,
        content: String,
        file: FileAssociation,
        notices: Vec<String>,
    ) {
        if let Some((pane, tab)) = self.find_tab_for_buffer(buffer_id) {
            self.activate_existing_tab(pane, tab);
        } else if self.editor.snapshot(buffer_id).is_some() {
            // Buffer still alive but its tab was closed — reopen it here.
            self.adopt_buffer_as_new_tab(buffer_id);
        } else {
            return;
        }
        self.bring_to_foreground();
        self.reconcile_file_buffer(buffer_id, content, file);
        // A non-empty notice (e.g. encoding) is decision-relevant — show it
        // as a sticky banner, overriding the reconcile's transient one.
        if let Some(text) = join_notices(&notices) {
            self.file_banner = Some(FileBanner::new(text));
        }
        invalidate_hwnd_with_reason(self.hwnd, "reveal_file_buffer_tab");
    }

    /// First `(pane, tab)` in this window whose tab shows `buffer_id`.
    fn find_tab_for_buffer(&self, buffer_id: BufferId) -> Option<(PaneId, TabId)> {
        self.tree.groups.iter().find_map(|(pane, group)| {
            group
                .tabs
                .iter()
                .find(|tid| {
                    self.tree
                        .tabs
                        .get(tid)
                        .is_some_and(|t| t.buffer_id == buffer_id)
                })
                .map(|tid| (*pane, *tid))
        })
    }

    fn activate_existing_tab(&mut self, pane: PaneId, tab: TabId) {
        if self.tree.focused != pane {
            self.switch_focus(pane);
        }
        if let Some(group) = self.tree.groups.get_mut(&pane) {
            group.activate(tab);
        }
        self.adopt_focused_tab();
        self.refresh_focused_viewport();
        self.refresh_language();
        self.maybe_submit_decoration();
    }

    fn bring_to_foreground(&self) {
        if self.hwnd.0 as isize == 0 {
            return;
        }
        unsafe {
            if IsIconic(self.hwnd).as_bool() {
                let _ = ShowWindow(self.hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(self.hwnd);
        }
    }
}

/// Collapse a notice list into one banner line: the first verbatim, with a
/// `(+ N more)` suffix when several arrived. Mirrors the launch-banner
/// collapse in [`crate::Window::new`]. Returns `None` for an empty list.
fn join_notices(notices: &[String]) -> Option<String> {
    let first = notices.first()?;
    if notices.len() > 1 {
        Some(format!("{first} (+ {} more)", notices.len() - 1))
    } else {
        Some(first.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::join_notices;

    #[test]
    fn empty_notices_yield_none() {
        assert_eq!(join_notices(&[]), None);
    }

    #[test]
    fn single_notice_is_verbatim() {
        assert_eq!(
            join_notices(&["encoding".to_string()]).as_deref(),
            Some("encoding")
        );
    }

    #[test]
    fn multiple_notices_collapse_with_count() {
        let notices = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(join_notices(&notices).as_deref(), Some("a (+ 2 more)"));
    }
}
