//! Tab-strip mouse interactions split from [`crate::window_mouse`].
//!
//! Covers tab-strip left/middle clicks, tab drag/drop hit-testing,
//! tab hover preview state, and the cross-window tab-drop fallback.
//! These methods are called from `window_mouse.rs`; pulling them out
//! keeps that file under the 600-line cap.

use continuity_render::{
    close_button_rect, tab_index_at, tab_slot_widths, TAB_CLOSE_MIN_TAB_WIDTH_DIP,
};
use windows::Win32::Foundation::{LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::Input::KeyboardAndMouse::SetCapture;
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetWindowRect, IsWindowVisible, PostMessageW, WM_USER,
};

use crate::mouse::{DropIndicator, TabDrag, TabDragPhase};
use crate::pane_layout::metrics;
use crate::pane_tree::PaneId;
use crate::window_mouse_hover::wall_clock_ms;
use crate::Window;

impl Window {
    pub(crate) fn try_cross_window_tab_drop(
        &mut self,
        drag: TabDrag,
        client_x: i32,
        client_y: i32,
    ) -> bool {
        let self_hwnd = self.hwnd;
        if self_hwnd.0 as isize == 0 {
            return false;
        }
        // Convert the up-event's client coords to screen so we can
        // hit-test other windows. Falls back to GetCursorPos if the
        // ClientToScreen call ever failed.
        let mut pt = POINT {
            x: client_x,
            y: client_y,
        };
        let translated = unsafe { ClientToScreen(self_hwnd, &mut pt) }.as_bool();
        if !translated {
            let mut cp = POINT::default();
            if unsafe { GetCursorPos(&mut cp) }.is_err() {
                return false;
            }
            pt = cp;
        }
        let cursor_screen = pt;
        let candidates = crate::window_registry::snapshot_others(self_hwnd);
        if candidates.is_empty() {
            return false;
        }
        let vd_manager = continuity_win::VirtualDesktopManager::new().ok();
        let my_vd = vd_manager
            .as_ref()
            .and_then(|m| m.desktop_id_of_window(self_hwnd));
        let target = candidates.into_iter().find(|cand| {
            if !unsafe { IsWindowVisible(*cand) }.as_bool() {
                return false;
            }
            // VD filter: skip windows on a different desktop. If either
            // side's desktop id is unknown, fall back to the permissive
            // "same desktop" assumption so the drop still works.
            if let (Some(mgr), Some(my)) = (vd_manager.as_ref(), my_vd) {
                if let Some(theirs) = mgr.desktop_id_of_window(*cand) {
                    if theirs != my {
                        return false;
                    }
                }
            }
            let mut rect = RECT::default();
            if unsafe { GetWindowRect(*cand, &mut rect) }.is_err() {
                return false;
            }
            cursor_screen.x >= rect.left
                && cursor_screen.x < rect.right
                && cursor_screen.y >= rect.top
                && cursor_screen.y < rect.bottom
        });
        let Some(target_hwnd) = target else {
            return false;
        };
        let Some(buffer_id) = self.tree.tabs.get(&drag.tab).map(|t| t.buffer_id) else {
            return false;
        };
        crate::window_registry::enqueue_adoption(target_hwnd, buffer_id);
        let _ = unsafe { PostMessageW(Some(target_hwnd), WM_USER + 1, WPARAM(0), LPARAM(0)) };
        // Source: remove the tab from the source pane (and collapse the
        // pane if it had only that one tab). We focus the source pane
        // briefly so `close_active_tab` operates on it.
        let prev_focused = self.tree.focused;
        if self.tree.groups.contains_key(&drag.pane) {
            if let Some(g) = self.tree.groups.get_mut(&drag.pane) {
                g.activate(drag.tab);
            }
            self.tree.focused = drag.pane;
            let _ = self.close_active_tab();
            if self.tree.groups.contains_key(&prev_focused) {
                self.tree.focused = prev_focused;
                self.adopt_focused_tab();
            }
        }
        true
    }

    pub(crate) fn try_tab_strip_left_down(&mut self, x: i32, y: i32) -> bool {
        let xf = x as f32;
        let yf = y as f32;
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
        self.switch_focus(pane);
        // Borrow the tabs immutably first to compute the strip layout
        // exactly the way the renderer paints it. Variable-width slots
        // matter: the previous equal-slot hit-test routed clicks to the
        // wrong tab once labels diverged in length.
        let (tab_ids, labels): (Vec<_>, Vec<_>) = {
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
        let Some(idx) = tab_index_at(&widths, xf - outer.x) else {
            self.clear_unsaved_close_arm();
            return true;
        };
        let tab = tab_ids[idx];

        // If the click landed on the close-button cell of a tab whose
        // close button is painted under the current mode, dispatch
        // `tab.close` for that tab instead of switching to it.
        let strip_h = metrics::TAB_STRIP_HEIGHT_DIP;
        let tab_x_local: f32 = widths.iter().take(idx).sum();
        let tab_w = widths.get(idx).copied().unwrap_or(0.0);
        let close_visible = crate::tab_hover::is_tab_close_visible(
            self.view_options.tab_close_button,
            self.mouse_state.tab_hover,
            pane,
            tab,
        );
        if close_visible && tab_w >= TAB_CLOSE_MIN_TAB_WIDTH_DIP {
            let rect = close_button_rect(outer.x + tab_x_local, tab_w, outer.y, strip_h);
            if xf >= rect.left && xf < rect.right && yf >= rect.top && yf < rect.bottom {
                // Briefly route through focus so `tab.close` operates on
                // the right tab even when it wasn't already active.
                self.switch_focus(pane);
                if let Some(group) = self.tree.groups.get_mut(&pane) {
                    group.activate(tab);
                }
                self.adopt_focused_tab();
                if !self.confirm_close_active_tab() {
                    return true;
                }
                let _ = self.close_active_tab();
                return true;
            }
        }

        self.clear_unsaved_close_arm();
        if let Some(group) = self.tree.groups.get_mut(&pane) {
            group.activate(tab);
        }
        self.adopt_focused_tab();
        let drag_label = self
            .tree
            .tabs
            .get(&tab)
            .map(|t| self.tab_label(t))
            .unwrap_or_default();
        let start_ms = crate::window_mouse_hover::wall_clock_ms();
        self.mouse_state.dragging = true;
        self.mouse_state.tab_drag = Some(crate::mouse::TabDrag {
            pane,
            tab,
            label: drag_label,
            start_x: x,
            start_y: y,
            start_ms,
            drop_indicator: None,
            resolution: crate::mouse::TabDropResolution::Cancel,
            phase: TabDragPhase::Armed,
        });
        // No ghost and no affordance at press time: the drag is `Armed`
        // and shows nothing until the cursor crosses the arm threshold.
        // Lifting the floating ghost only happens once the phase machine
        // reaches `Detached` (see `on_tab_drag_mouse_move`).
        crate::paint_trace::log_event(
            "tab_drag",
            "state=start target=cancel slot=-1 foreign_hwnd=0 elapsed_ms_since_start=0",
        );
        // Capture the mouse so the user can drop the tab anywhere on
        // screen — including over another Continuity window on the same
        // virtual desktop — and our `WM_LBUTTONUP` still fires.
        if self.hwnd.0 as isize != 0 {
            unsafe {
                let _ = SetCapture(self.hwnd);
            }
        }
        true
    }

    /// `WM_MBUTTONDOWN`: tab-strip middle-click closes the tab under
    /// the cursor. Middle-click on the strip's empty trailing area
    /// opens a fresh tab in that pane. Outside the tab strip the click
    /// is ignored.
    pub(crate) fn on_middle_button_down(&mut self, x: i32, y: i32) -> bool {
        self.try_tab_strip_middle_down(x, y)
    }

    /// Mid-drag mouse-move handler split out so [`Self::on_mouse_move`]
    /// stays under the conventions cap. Recomputes the live drop
    /// resolution, fires per-transition trace + cross-window broadcast,
    /// and reports whether the paint should be invalidated.
    pub(crate) fn on_tab_drag_mouse_move(&mut self, x: i32, y: i32) -> bool {
        let Some(delta) = self.refresh_tab_drag_resolution(x, y) else {
            return false;
        };
        // The floating ghost is a `Detached`-only affordance — while the
        // drag is still grounded in the strip (`Armed` / `Reorder`) the
        // tab is not lifted. `update_tab_drag_lift_state` already destroys the
        // ghost on the transition out of `Detached`, so this only has to
        // (re)show it while detached.
        if self
            .mouse_state
            .tab_drag
            .as_ref()
            .is_some_and(|drag| drag.phase.is_detached())
        {
            self.update_tab_drag_ghost_at_client_point(x, y);
        }
        if delta.variant_changed {
            if let crate::mouse::TabDropResolution::ForeignWindow { hwnd_raw } = delta.previous {
                self.send_tab_drag_leave(hwnd_raw);
            }
            let drag =
                self.mouse_state.tab_drag.as_ref().expect(
                    "invariant: refresh_tab_drag_resolution succeeded only when drag was set",
                );
            let elapsed = wall_clock_ms().saturating_sub(drag.start_ms);
            let phase = if drag.phase.is_detached() {
                "detached"
            } else {
                "reorder"
            };
            let foreign = match delta.current {
                crate::mouse::TabDropResolution::ForeignWindow { hwnd_raw } => hwnd_raw as u64,
                _ => 0,
            };
            let slot = match delta.current {
                crate::mouse::TabDropResolution::SourceStrip(i) => i.slot as i32,
                _ => -1,
            };
            crate::paint_trace::log_event(
                "tab_drag",
                &format!(
                    "state=over target={target} phase={phase} slot={slot} \
                     foreign_hwnd={foreign} elapsed_ms_since_start={elapsed}",
                    target = delta.current.as_trace_str(),
                ),
            );
        }
        if crate::window_tab_drag::should_broadcast_foreign_hover(delta.current) {
            if let crate::mouse::TabDropResolution::ForeignWindow { hwnd_raw } = delta.current {
                self.send_tab_drag_hover(hwnd_raw, x, y);
            }
        }
        delta.variant_changed || delta.indicator_changed
    }

    fn try_tab_strip_middle_down(&mut self, x: i32, y: i32) -> bool {
        let xf = x as f32;
        let yf = y as f32;
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
        let (tab_ids, labels): (Vec<_>, Vec<_>) = {
            let Some(group) = self.tree.groups.get(&pane) else {
                return false;
            };
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
        match tab_index_at(&widths, xf - outer.x) {
            Some(idx) => {
                let tab = tab_ids[idx];
                self.switch_focus(pane);
                if let Some(group) = self.tree.groups.get_mut(&pane) {
                    group.activate(tab);
                }
                self.adopt_focused_tab();
                if !self.confirm_close_active_tab() {
                    return true;
                }
                let _ = self.close_active_tab();
                true
            }
            None => {
                self.switch_focus(pane);
                let _ = self.dispatch_command("tab.new", &serde_json::Value::Null);
                true
            }
        }
    }

    /// Resolve client `(x, y)` to the tab strip entry under the cursor,
    /// if any. `None` outside any pane's strip OR past the rightmost tab.
    pub(crate) fn tab_at_position(
        &self,
        x: i32,
        y: i32,
    ) -> Option<(PaneId, crate::pane_tree::TabId)> {
        let xf = x as f32;
        let yf = y as f32;
        let (pane, outer) = self
            .pane_outer_rects()
            .into_iter()
            .find(|(_, r)| r.contains(xf, yf))?;
        if yf >= outer.y + metrics::TAB_STRIP_HEIGHT_DIP {
            return None;
        }
        let group = self.tree.groups.get(&pane)?;
        if group.tabs.is_empty() {
            return None;
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
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let widths = tab_slot_widths(&label_refs, outer.w);
        let idx = tab_index_at(&widths, xf - outer.x)?;
        Some((pane, group.tabs[idx]))
    }

    /// Refresh the hover slot from a fresh `(x, y)`. Returns true when
    /// the slot's contents changed so the caller can invalidate.
    pub(crate) fn update_tab_hover_from_pixel(&mut self, x: i32, y: i32) -> bool {
        let now_ms = wall_clock_ms();
        let over = self.tab_at_position(x, y);
        crate::tab_hover::update_tab_hover(&mut self.mouse_state.tab_hover, over, now_ms)
    }

    /// Clear any in-flight tab hover. Wired into the Esc dismiss chain
    /// and every overlay-open path.
    pub(crate) fn clear_tab_hover(&mut self) -> bool {
        self.mouse_state.tab_hover.take().is_some()
    }

    /// Compute the live drop indicator slot for a tab drag at client
    /// `(x, y)`. Returns `None` when the cursor is not over any pane's
    /// tab strip.
    pub(crate) fn compute_tab_drop_indicator(&self, x: i32, y: i32) -> Option<DropIndicator> {
        let xf = x as f32;
        let yf = y as f32;
        let (pane, outer) = self
            .pane_outer_rects()
            .into_iter()
            .find(|(_, r)| r.contains(xf, yf))?;
        if yf >= outer.y + metrics::TAB_STRIP_HEIGHT_DIP {
            return None;
        }
        let group = self.tree.groups.get(&pane)?;
        if group.tabs.is_empty() {
            return Some(DropIndicator { pane, slot: 0 });
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
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let widths = tab_slot_widths(&label_refs, outer.w);
        let x_in_strip = xf - outer.x;
        let slot = tab_drop_slot(&widths, x_in_strip);
        Some(DropIndicator { pane, slot })
    }

    /// Clone the buffer behind `tab` into `target_pane` as a fresh tab.
    /// Used by Ctrl+drag in cross-pane drops: the source tab stays put,
    /// the new tab opens the same `BufferId`. Focuses the new tab.
    pub(crate) fn clone_tab_to_pane(
        &mut self,
        tab: crate::pane_tree::TabId,
        target_pane: PaneId,
    ) -> Option<()> {
        let buffer_id = self.tree.tabs.get(&tab)?.buffer_id;
        if !self.tree.groups.contains_key(&target_pane) {
            return None;
        }
        self.tree.focused = target_pane;
        self.adopt_buffer_as_new_tab(buffer_id);
        Some(())
    }
}

/// Given tab `widths` and a cursor x relative to the strip, return the
/// drop slot (insertion index into the tabs vector). Each tab is split
/// in half: the left half points at "insert before this tab", the
/// right half points at "insert after". A cursor past the rightmost tab
/// targets `widths.len()` (insert at the end). Empty strips return `0`.
#[must_use]
pub(crate) fn tab_drop_slot(widths: &[f32], x_in_strip: f32) -> usize {
    if widths.is_empty() {
        return 0;
    }
    if x_in_strip <= 0.0 {
        return 0;
    }
    let mut acc = 0.0;
    for (i, w) in widths.iter().enumerate() {
        let next = acc + *w;
        if x_in_strip < next {
            let mid = acc + *w * 0.5;
            return if x_in_strip < mid { i } else { i + 1 };
        }
        acc = next;
    }
    widths.len()
}

#[cfg(test)]
mod drop_slot_tests {
    use super::tab_drop_slot;

    #[test]
    fn empty_strip_returns_zero() {
        assert_eq!(tab_drop_slot(&[], 0.0), 0);
        assert_eq!(tab_drop_slot(&[], 50.0), 0);
    }

    #[test]
    fn negative_x_returns_zero() {
        assert_eq!(tab_drop_slot(&[100.0, 100.0], -5.0), 0);
    }

    #[test]
    fn left_half_of_first_tab_returns_zero() {
        assert_eq!(tab_drop_slot(&[100.0, 100.0], 10.0), 0);
        assert_eq!(tab_drop_slot(&[100.0, 100.0], 49.9), 0);
    }

    #[test]
    fn right_half_of_first_tab_returns_one() {
        assert_eq!(tab_drop_slot(&[100.0, 100.0], 50.0), 1);
        assert_eq!(tab_drop_slot(&[100.0, 100.0], 99.9), 1);
    }

    #[test]
    fn left_half_of_second_tab_returns_one() {
        assert_eq!(tab_drop_slot(&[100.0, 100.0], 100.0), 1);
        assert_eq!(tab_drop_slot(&[100.0, 100.0], 149.9), 1);
    }

    #[test]
    fn right_half_of_last_tab_returns_len() {
        assert_eq!(tab_drop_slot(&[100.0, 100.0], 150.0), 2);
        assert_eq!(tab_drop_slot(&[100.0, 100.0], 199.9), 2);
    }

    #[test]
    fn past_strip_returns_len() {
        assert_eq!(tab_drop_slot(&[100.0, 100.0], 250.0), 2);
        assert_eq!(tab_drop_slot(&[60.0], 1000.0), 1);
    }

    #[test]
    fn variable_widths_respect_midpoint() {
        let widths = [40.0, 200.0];
        assert_eq!(tab_drop_slot(&widths, 19.0), 0);
        assert_eq!(tab_drop_slot(&widths, 21.0), 1);
        assert_eq!(tab_drop_slot(&widths, 139.0), 1);
        assert_eq!(tab_drop_slot(&widths, 141.0), 2);
    }
}
