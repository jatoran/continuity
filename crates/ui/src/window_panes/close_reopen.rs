//! Tab close / reopen-closed orchestration on `Window`.
//!
//! Lifted out of `window_panes.rs` to stay under the 600-line cap and
//! to keep the close → push-recently_closed → reopen-from-stack flow
//! adjacent. The reopen path verifies the recorded buffer is still
//! adopted in `EditorHandle`, skips phantom entries instead of
//! installing a tab pointing at a non-existent buffer, and routes the
//! reopened tab back to its origin pane when that pane is still alive.
//!
//! Trace events emitted here:
//! - `event:tab_close pane=… buffer=… tab=… label_len=N \
//!   recently_closed_len=N after=sibling_tab|pane_collapse|window_close`
//! - `event:tab_reopen outcome=ok|empty_stack|phantom_buffer_skip|\
//!   exhausted_after_skips …`
//!
//! Thread ownership: UI thread of one window.

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};

use crate::pane_layout::parent_split::find_parent_split_info;
use crate::pane_tree::{ClosedTab, PaneId, TabId};
use crate::window::Window;
use crate::Error;

impl Window {
    /// Close the active tab in the focused group. Empty group → close
    /// the pane. Buffer goes to trash (Phase 13: every buffer is
    /// non-file-associated until Phase 15).
    pub(crate) fn close_active_tab(&mut self) -> Result<(), Error> {
        crate::window_buffer_tab_repair::repair_pane_tree_structure(&mut self.tree, &self.editor);
        self.save_current_right_edge_chrome_state();
        let focused = self.tree.focused;
        let active = match self.tree.groups.get(&focused) {
            Some(g) => g.active,
            None => return Ok(()),
        };
        let tab = match self.tree.tabs.get(&active) {
            Some(t) => t.clone(),
            None => return Ok(()),
        };
        let label = self.tab_label(&tab);
        let now = self.now_ms();

        // Capture parent-split shape BEFORE the close mutates the
        // tree so reopen can re-split on the same axis when the
        // origin pane gets collapsed by this close.
        let parent_info = find_parent_split_info(&self.tree.root, focused);

        // Remove from group; if group empties, close the pane.
        let next_in_group = if let Some(g) = self.tree.groups.get_mut(&focused) {
            g.remove_tab(active)
        } else {
            None
        };
        self.tree.tabs.remove(&active);
        // G2: drop any per-buffer find-bar memory for the closed buffer.
        // Find state is in-memory only and never persists across a
        // buffer's lifecycle.
        self.forget_find_memory(tab.buffer_id);
        // Record in recently-closed with the origin pane so reopen can
        // route the tab back to where the user closed it.
        let recorded_label = label.clone();
        self.tree.recently_closed.insert(
            0,
            ClosedTab {
                buffer_id: tab.buffer_id,
                label,
                closed_at_ms: now,
                origin_pane: Some(focused),
                parent_split_axis: parent_info.map(|p| p.axis),
                parent_sibling_leaf: parent_info.and_then(|p| p.sibling_leaf),
            },
        );
        const RECENTLY_CLOSED_CAP: usize = 32;
        if self.tree.recently_closed.len() > RECENTLY_CLOSED_CAP {
            self.tree.recently_closed.truncate(RECENTLY_CLOSED_CAP);
        }
        let next_in_group_token = if next_in_group.is_some() {
            "sibling_tab"
        } else if self.tree.root.leaf_ids().len() <= 1 {
            "window_close"
        } else {
            "pane_collapse"
        };
        if crate::paint_trace::is_trace_enabled() {
            crate::paint_trace::log_event(
                "tab_close",
                &format!(
                    "pane={} buffer={} tab={} label_len={} recently_closed_len={} after={}",
                    focused.0,
                    tab.buffer_id.as_uuid(),
                    active.0,
                    recorded_label.chars().count(),
                    self.tree.recently_closed.len(),
                    next_in_group_token,
                ),
            );
        }

        if next_in_group.is_none() {
            // D4: closing the last tab exits the window. Preserve the
            // user's "all tabs closed" intent by saving a clean untitled
            // tab for the next launch instead of an invalid empty group
            // or the just-closed file.
            if self.tree.root.leaf_ids().len() <= 1 {
                self.install_blank_restore_tab(focused, now);
                if self.hwnd.0 as isize != 0 {
                    let _ =
                        unsafe { PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) };
                }
            } else {
                self.close_focused_pane()?;
            }
        } else {
            self.adopt_focused_tab();
        }
        let _ = now;
        self.request_state_save();
        Ok(())
    }

    fn install_blank_restore_tab(&mut self, pane: PaneId, now: u64) {
        let buffer_id = self.editor.open_buffer(String::new());
        let tab_id = self.tree.insert_fresh_buffer_tab(buffer_id, now);
        if let Some(group) = self.tree.groups.get_mut(&pane) {
            group.tabs.clear();
            group.tabs.push(tab_id);
            group.active = tab_id;
            group.mru.clear();
            group.mru.push(tab_id);
        }
        self.buffer_id = buffer_id;
        self.view = continuity_layout::ViewState::new();
        self.apply_right_edge_chrome_for_current_view();
        self.panes.remove(&pane);
    }

    /// Reopen the most-recently-closed tab. Validates the recorded
    /// buffer is still adoptable, prefers the originating pane when it
    /// still exists, and skips entries whose buffer cannot be resolved
    /// instead of inserting a phantom tab. Returns the new tab id or
    /// `None` when no usable entry was found.
    pub(crate) fn reopen_closed_tab(&mut self) -> Option<TabId> {
        self.cancel_scroll_inertia();
        let trace_on = crate::paint_trace::is_trace_enabled();
        let initial_stack_len = self.tree.recently_closed.len();
        let mut skipped: u32 = 0;
        if self.tree.recently_closed.is_empty() {
            if trace_on {
                crate::paint_trace::log_event(
                    "tab_reopen",
                    "outcome=empty_stack stack_before=0 stack_after=0",
                );
            }
            return None;
        }
        // Pop entries until we find one whose buffer is still
        // resolvable in the editor. The previous implementation took
        // the head unconditionally and emitted a tab pointing at a
        // phantom buffer; that produced the "Ctrl+W to recover" blank
        // window.
        loop {
            let entry = match self.tree.recently_closed.first().cloned() {
                Some(e) => e,
                None => {
                    if trace_on {
                        crate::paint_trace::log_event(
                            "tab_reopen",
                            &format!(
                                "outcome=exhausted_after_skips skipped_phantoms={} \
                                 stack_before={} stack_after=0",
                                skipped, initial_stack_len,
                            ),
                        );
                    }
                    return None;
                }
            };
            // Probe the buffer. `EditorHandle::snapshot` returns `None`
            // when the buffer is not adopted in EditorState — the
            // canonical phantom-buffer signal.
            let buffer_alive = self.editor.snapshot(entry.buffer_id).is_some();
            if !buffer_alive {
                self.tree.recently_closed.remove(0);
                skipped = skipped.saturating_add(1);
                if trace_on {
                    crate::paint_trace::log_event(
                        "tab_reopen",
                        &format!(
                            "outcome=phantom_buffer_skip buffer={} label_len={} \
                             skipped_so_far={} stack_remaining={}",
                            entry.buffer_id.as_uuid(),
                            entry.label.chars().count(),
                            skipped,
                            self.tree.recently_closed.len(),
                        ),
                    );
                }
                continue;
            }
            self.tree.recently_closed.remove(0);
            let origin_alive = entry
                .origin_pane
                .map(|p| self.tree.groups.contains_key(&p))
                .unwrap_or(false);
            // When the origin is collapsed, see if we can resurrect a
            // pane in roughly its original position by re-splitting
            // the recorded sibling leaf along the recorded axis.
            let sibling_alive = entry
                .parent_sibling_leaf
                .map(|p| self.tree.groups.contains_key(&p))
                .unwrap_or(false);
            let can_restore_via_split =
                !origin_alive && sibling_alive && entry.parent_split_axis.is_some();
            let now = self.now_ms();
            let id = self.tree.insert_fresh_buffer_tab(entry.buffer_id, now);

            let (destination_pane, dest_token) = if origin_alive {
                let dest = entry.origin_pane.expect("invariant: checked above");
                (dest, "origin_pane")
            } else if can_restore_via_split {
                let sibling = entry.parent_sibling_leaf.expect("invariant: checked above");
                let axis = entry.parent_split_axis.expect("invariant: checked above");
                // Mint a new pane id by creating a single-tab Group
                // and splitting the sibling along the recorded axis.
                let mut new_group = crate::pane_tree::Group::singleton_with_id(
                    self.tree.fresh_unused_pane_id(),
                    id,
                );
                let new_pane = new_group.id;
                new_group.active = id;
                self.tree.groups.insert(new_pane, new_group);
                if crate::pane_layout::splice_split_at_pane(&mut self.tree, sibling, axis, new_pane)
                {
                    self.tree.maximized = None;
                    (new_pane, "restored_via_resplit")
                } else {
                    // Shouldn't normally hit — sibling was alive a
                    // moment ago — but degrade gracefully to focused.
                    self.tree.groups.remove(&new_pane);
                    (self.tree.focused, "fallback_split_failed")
                }
            } else if entry.origin_pane.is_some() {
                (self.tree.focused, "fallback_origin_collapsed")
            } else {
                (self.tree.focused, "fallback_no_origin")
            };

            // Move focus to the destination pane before pushing the
            // tab + rebinding window state — `g.push_tab(id, true)`
            // sets `g.active = id`, so painting at the new pane needs
            // `Window::buffer_id` / `view` aligned with that pane's
            // active tab.
            if destination_pane != self.tree.focused {
                self.switch_focus(destination_pane);
            }
            // Restored-via-resplit panes already hold this tab as the
            // group's active member. Existing panes need an explicit
            // push to add the tab + bump it to active.
            if dest_token != "restored_via_resplit" {
                if let Some(g) = self.tree.groups.get_mut(&destination_pane) {
                    g.push_tab(id, true);
                }
            }
            self.save_current_right_edge_chrome_state();
            self.apply_new_pane_state(entry.buffer_id);
            self.refresh_focused_viewport();
            self.refresh_language();
            self.maybe_submit_decoration();
            let _ = self.try_dispatch_projection_worker_early("reopen_closed_tab", "focus_change");
            self.request_state_save();
            if trace_on {
                crate::paint_trace::log_event(
                    "tab_reopen",
                    &format!(
                        "outcome=ok pane={} dest={} buffer={} tab={} \
                         skipped_phantoms={} stack_before={} stack_after={}",
                        destination_pane.0,
                        dest_token,
                        entry.buffer_id.as_uuid(),
                        id.0,
                        skipped,
                        initial_stack_len,
                        self.tree.recently_closed.len(),
                    ),
                );
            }
            return Some(id);
        }
    }
}
