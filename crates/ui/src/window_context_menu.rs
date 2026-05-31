//! Umbrella `WM_CONTEXTMENU` dispatcher: non-text chrome context menus,
//! tab-strip / pane-chrome menus, then spell suggestions as fallback.
//!
//! Thread ownership: UI thread (the only owner of the HWND, the pane
//! tree, and `TrackPopupMenu`'s thread-affinity).

use continuity_command::{
    MARKDOWN_TABLE_DELETE_COL, MARKDOWN_TABLE_DELETE_ROW, MARKDOWN_TABLE_DELETE_TABLE,
    MARKDOWN_TABLE_INSERT_COL_LEFT, MARKDOWN_TABLE_INSERT_COL_RIGHT,
    MARKDOWN_TABLE_INSERT_ROW_ABOVE, MARKDOWN_TABLE_INSERT_ROW_BELOW, PANE_SPLIT_HORIZONTAL,
    PANE_SPLIT_VERTICAL, TAB_CLOSE, TAB_NEW, VIEW_TOGGLE_FILE_TREE, VIEW_TOGGLE_MINIMAP,
    WINDOW_NEW_WINDOW,
};
use continuity_render::{tab_index_at, tab_slot_widths};
use serde_json::Value;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

use crate::pane_layout::metrics;
use crate::pane_tree::TabId;
use crate::window_right_edge_chrome::ChromeContextTarget;
use crate::Window;

mod menu;
use menu::{
    track_chrome_toggle_menu, track_menu, track_table_cell_menu, ID_CHROME_TOGGLE,
    ID_PANE_SPLIT_HORIZONTAL, ID_PANE_SPLIT_VERTICAL, ID_TABLE_DELETE_COL, ID_TABLE_DELETE_ROW,
    ID_TABLE_DELETE_TABLE, ID_TABLE_INSERT_COL_LEFT, ID_TABLE_INSERT_COL_RIGHT,
    ID_TABLE_INSERT_ROW_ABOVE, ID_TABLE_INSERT_ROW_BELOW, ID_TABLE_TOGGLE_WRAP, ID_TAB_CLOSE,
    ID_TAB_NEW, ID_WINDOW_NEW,
};

impl Window {
    /// Route `WM_CONTEXTMENU`. Decode the lparam point, try chrome menus,
    /// then fall back to the spell-suggestion popup. Returns `true` when
    /// the message was consumed.
    pub(crate) fn on_context_menu(&mut self, hwnd: HWND, lparam: isize) -> bool {
        // Decode the screen-pixel coords from `lparam`. The Win32 special
        // value `0xFFFFFFFF` signals a keyboard-driven invocation (Shift+
        // F10 / VK_APPS) — fall back to the cursor position so a keyboard
        // user gets a menu near the cursor rather than at (0,0).
        let raw = lparam as u32;
        let (screen_x, screen_y, keyboard) = if raw == 0xFFFF_FFFF {
            let mut pt = POINT::default();
            if unsafe { GetCursorPos(&mut pt) }.is_ok() {
                (pt.x, pt.y, true)
            } else {
                (0, 0, true)
            }
        } else {
            (
                (raw & 0xFFFF) as i16 as i32,
                ((raw >> 16) & 0xFFFF) as i16 as i32,
                false,
            )
        };
        let mut pt = POINT {
            x: screen_x,
            y: screen_y,
        };
        if unsafe { ScreenToClient(hwnd, &mut pt) }.as_bool() {
            if self.try_chrome_context_menu(hwnd, screen_x, screen_y, pt.x, pt.y) {
                return true;
            }
            // Tab-strip / pane chrome right-click → tab/pane/window menu.
            if self.try_tab_strip_context_menu(hwnd, screen_x, screen_y, pt.x, pt.y) {
                return true;
            }
            // Pipe-table cell right-click → row/column structural ops.
            if self.try_table_cell_context_menu(hwnd, screen_x, screen_y, pt.x, pt.y) {
                return true;
            }
        }
        // Keyboard invocations always anchor on the caret for spell
        // suggestions; mouse invocations use the click point indirectly
        // (the spell popup itself anchors on the caret pixel).
        let _ = keyboard;
        self.spell_on_context_menu((pt.x, pt.y))
    }

    fn try_chrome_context_menu(
        &mut self,
        hwnd: HWND,
        screen_x: i32,
        screen_y: i32,
        client_x: i32,
        client_y: i32,
    ) -> bool {
        let Some(target) = self.chrome_context_target_at(client_x as f32, client_y as f32) else {
            return false;
        };
        let (label, command_id, target_pane) = match target {
            ChromeContextTarget::FileTree => {
                ("Toggle File Tree", VIEW_TOGGLE_FILE_TREE.as_str(), None)
            }
            ChromeContextTarget::Minimap { pane_id } => (
                "Toggle Minimap",
                VIEW_TOGGLE_MINIMAP.as_str(),
                Some(pane_id),
            ),
            ChromeContextTarget::Outline { pane_id } => (
                "Toggle Outline",
                continuity_command::view::VIEW_TOGGLE_OUTLINE.as_str(),
                Some(pane_id),
            ),
        };
        let chosen = unsafe {
            track_chrome_toggle_menu(hwnd, screen_x, screen_y, &self.keymap, label, command_id)
        };
        if chosen == ID_CHROME_TOGGLE {
            if let Some(pane_id) = target_pane {
                self.switch_focus(pane_id);
            }
            let _ = self.dispatch_command(command_id, &Value::Null);
        }
        true
    }

    /// Build and track the tab/pane/window context menu when `(client_x,
    /// client_y)` falls inside a tab strip. Returns `true` when the menu
    /// was shown.
    fn try_tab_strip_context_menu(
        &mut self,
        hwnd: HWND,
        screen_x: i32,
        screen_y: i32,
        client_x: i32,
        client_y: i32,
    ) -> bool {
        let xf = client_x as f32;
        let yf = client_y as f32;
        let Some((pane, outer)) = self
            .pane_outer_rects()
            .into_iter()
            .find(|(_, r)| r.contains(xf, yf))
        else {
            return false;
        };
        if yf >= outer.y + metrics::TAB_STRIP_HEIGHT_DIP {
            return false;
        }
        // Resolve the right-clicked tab using the same variable-width
        // hit-test the left-click path uses, so labels of different
        // lengths route correctly.
        let (tab_ids, labels) = {
            let Some(group) = self.tree.groups.get(&pane) else {
                return false;
            };
            if group.tabs.is_empty() {
                return false;
            }
            let labels: Vec<String> = group
                .tabs
                .iter()
                .map(|tid| {
                    self.tree
                        .tabs
                        .get(tid)
                        .map(|t| self.tab_label(t))
                        .unwrap_or_default()
                })
                .collect();
            (group.tabs.clone(), labels)
        };
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let widths = tab_slot_widths(&label_refs, outer.w);
        let tab_idx = tab_index_at(&widths, xf - outer.x).unwrap_or(tab_ids.len() - 1);
        let target_tab = tab_ids[tab_idx];

        // Right-click should target the clicked pane/tab regardless of
        // current focus. Switch focus + activate the target tab so the
        // dispatched commands operate on what the user pointed at.
        let prior_focus = self.tree.focused;
        let prior_active = self
            .tree
            .groups
            .get(&pane)
            .map(|g| g.active)
            .unwrap_or(target_tab);
        if pane != prior_focus {
            self.switch_focus(pane);
        }
        if let Some(group) = self.tree.groups.get_mut(&pane) {
            group.activate(target_tab);
        }
        self.adopt_focused_tab();

        let chosen = unsafe { track_menu(hwnd, screen_x, screen_y, &self.keymap) };

        match chosen {
            ID_TAB_NEW => {
                let _ = self.dispatch_command(TAB_NEW.as_str(), &Value::Null);
            }
            ID_WINDOW_NEW => {
                let _ = self.dispatch_command(WINDOW_NEW_WINDOW.as_str(), &Value::Null);
            }
            ID_TAB_CLOSE => {
                // `tab.close` (dispatched here) runs its own
                // [`Window::confirm_close_active_tab`] prompt, so we don't
                // pre-prompt — that would double up the dialog. If the
                // user cancels at the prompt, `target_tab` survives in
                // the tree; restore the prior focused pane + active tab
                // so the right-click had no net effect.
                let _ = self.dispatch_command(TAB_CLOSE.as_str(), &Value::Null);
                if self.tree.tabs.contains_key(&target_tab) {
                    self.restore_tab_focus(pane, prior_active, prior_focus);
                }
            }
            ID_PANE_SPLIT_VERTICAL => {
                let _ = self.dispatch_command(PANE_SPLIT_HORIZONTAL.as_str(), &Value::Null);
            }
            ID_PANE_SPLIT_HORIZONTAL => {
                let _ = self.dispatch_command(PANE_SPLIT_VERTICAL.as_str(), &Value::Null);
            }
            _ => {
                // Menu dismissed without a selection — restore prior focus
                // so a right-click that the user cancelled doesn't reorder
                // the MRU stack.
                self.restore_tab_focus(pane, prior_active, prior_focus);
            }
        }
        true
    }

    /// Right-click inside a visual table cell: place the caret on the
    /// clicked cell (so the dispatched commands operate on the right
    /// row/column), then track a popup menu of row/column/table ops.
    /// Returns `true` when the click hit a table cell, regardless of
    /// whether the user selected an item.
    fn try_table_cell_context_menu(
        &mut self,
        hwnd: HWND,
        screen_x: i32,
        screen_y: i32,
        client_x: i32,
        client_y: i32,
    ) -> bool {
        // Hit-test against the cached visual cell rects. The hit-test
        // helper consults `last_focused_table_layouts`, which is
        // written by the focused-pane paint loop, so this works
        // regardless of whether decorate is currently lagging.
        let pos = match self.client_to_buffer_position(client_x, client_y) {
            Some(p) => p,
            None => return false,
        };
        let snap = match self.editor.snapshot(self.buffer_id) {
            Some(s) => s,
            None => return false,
        };
        let rope = snap.rope_snapshot().rope();
        let line = pos.line as usize;
        let line_start = if line < rope.len_lines() {
            rope.line_to_byte(line)
        } else {
            rope.len_bytes()
        };
        let caret_byte = line_start + pos.byte_in_line as usize;
        let id = self.buffer_id.as_uuid().as_u128();
        let in_table = self
            .decoration_cache
            .get(id)
            .map(|dec| {
                dec.evaluated_tables
                    .iter()
                    .any(|t| caret_byte >= t.block_range.start && caret_byte < t.block_range.end)
            })
            .unwrap_or(false);
        if !in_table {
            return false;
        }
        // Move the caret to the right-clicked cell so the dispatched
        // structural commands target the cell under the cursor, not
        // wherever the caret used to be.
        let _ = self.place_caret_at_pixel(client_x, client_y, false);

        let chosen = unsafe { track_table_cell_menu(hwnd, screen_x, screen_y, &self.keymap) };
        match chosen {
            ID_TABLE_INSERT_ROW_ABOVE => {
                let _ =
                    self.dispatch_command(MARKDOWN_TABLE_INSERT_ROW_ABOVE.as_str(), &Value::Null);
            }
            ID_TABLE_INSERT_ROW_BELOW => {
                let _ =
                    self.dispatch_command(MARKDOWN_TABLE_INSERT_ROW_BELOW.as_str(), &Value::Null);
            }
            ID_TABLE_INSERT_COL_LEFT => {
                let _ =
                    self.dispatch_command(MARKDOWN_TABLE_INSERT_COL_LEFT.as_str(), &Value::Null);
            }
            ID_TABLE_INSERT_COL_RIGHT => {
                let _ =
                    self.dispatch_command(MARKDOWN_TABLE_INSERT_COL_RIGHT.as_str(), &Value::Null);
            }
            ID_TABLE_DELETE_ROW => {
                let _ = self.dispatch_command(MARKDOWN_TABLE_DELETE_ROW.as_str(), &Value::Null);
            }
            ID_TABLE_DELETE_COL => {
                let _ = self.dispatch_command(MARKDOWN_TABLE_DELETE_COL.as_str(), &Value::Null);
            }
            ID_TABLE_DELETE_TABLE => {
                let _ = self.dispatch_command(MARKDOWN_TABLE_DELETE_TABLE.as_str(), &Value::Null);
            }
            ID_TABLE_TOGGLE_WRAP => {
                let _ = self.markdown_table_toggle_wrap_impl();
            }
            _ => {}
        }
        true
    }

    fn restore_tab_focus(
        &mut self,
        pane: crate::pane_tree::PaneId,
        tab: TabId,
        focused: crate::pane_tree::PaneId,
    ) {
        if let Some(group) = self.tree.groups.get_mut(&pane) {
            group.activate(tab);
        }
        if self.tree.focused != focused {
            self.switch_focus(focused);
        }
        self.adopt_focused_tab();
    }
}
