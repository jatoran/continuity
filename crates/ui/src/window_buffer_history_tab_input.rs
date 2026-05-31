//! Mouse / wheel / keyboard dispatch for the buffer-history tab
//! surface. Sibling of `window_buffer_history_tab.rs`. Pulled out so
//! the parent file stays under the 600-line cap.
//!
//! Surface: each entry point returns `true` when the event was claimed
//! by the history tab and the standard chrome / caret-placement paths
//! should be skipped.

use continuity_render::{buffer_history_hit_test_lane, compute_buffer_history_panel_layout};

use crate::buffer_history_tab::PanDragState;
use crate::pane_tree_kind::TabKind;
use crate::Window;

impl Window {
    /// `WM_LBUTTONDOWN` for the buffer-history surface. Click on a
    /// lane row commits that buffer; click on the time-ruler band or
    /// empty area starts a viewport-pan drag. Returns `true` when the
    /// click was consumed so the normal caret-placement path skips it.
    pub(crate) fn try_buffer_history_left_down(&mut self, x: i32, y: i32) -> bool {
        if !self.focused_tab_is_buffer_history() {
            return false;
        }
        let tab_id = match self.tree.active_tab() {
            Some(t) if t.kind == TabKind::BufferHistory => t.id,
            _ => return false,
        };
        let rect = self.focused_buffer_history_panel_rect();
        // Click outside the panel body (e.g. on the tab strip or
        // status bar) falls through to the chrome's own handlers.
        if (x as f32) < rect.x
            || (x as f32) > rect.x + rect.w
            || (y as f32) < rect.y
            || (y as f32) > rect.y + rect.h
        {
            return false;
        }
        let draw = self.build_buffer_history_panel_draw(rect);
        let layout = compute_buffer_history_panel_layout(&draw);
        let strip_width = layout
            .lanes
            .first()
            .map(|l| l.strip_rect.w)
            .unwrap_or(rect.w);
        if let Some(lane_idx) = buffer_history_hit_test_lane(&layout, x as f32, y as f32) {
            if let Some(state) = self.buffer_history_tabs.get_mut(&tab_id) {
                // Sync selection AND hover to the just-clicked lane.
                // The preview band reads from `hovered_lane` first;
                // if the cursor hadn't moved since opening the tab
                // (or since the last wheel-scroll), `hovered_lane`
                // was stale and showed a different row's content
                // than the row the click actually hit. Pinning both
                // here means the preview the user just saw is
                // guaranteed to be the buffer that opens.
                state.selected_lane = Some(lane_idx);
                state.hovered_lane = Some(lane_idx);
            }
            self.confirm_buffer_history_tab_selection(tab_id);
            return true;
        }
        // Click outside any lane row (ruler band, strip background,
        // empty area below the last lane) starts a drag-pan. The
        // viewport translates relative to this captured snapshot
        // until WM_LBUTTONUP clears `pan_drag`.
        if let Some(state) = self.buffer_history_tabs.get_mut(&tab_id) {
            state.pan_drag = Some(PanDragState {
                start_client_x: x,
                start_viewport_start_ms: state.viewport_start_ms,
                start_viewport_end_ms: state.viewport_end_ms,
                strip_width_dip: strip_width.max(1.0),
            });
        }
        true
    }

    /// `WM_LBUTTONUP` for the buffer-history surface. Clears the
    /// active pan-drag state (if any). Returns `true` when a drag
    /// was in progress so the standard caret-deselect path is not
    /// also triggered.
    pub(crate) fn on_buffer_history_left_button_up(&mut self) -> bool {
        let tab_id = match self.tree.active_tab() {
            Some(t) if t.kind == TabKind::BufferHistory => t.id,
            _ => return false,
        };
        if let Some(state) = self.buffer_history_tabs.get_mut(&tab_id) {
            if state.pan_drag.take().is_some() {
                self.request_repaint();
                return true;
            }
        }
        false
    }

    /// `WM_MOUSEWHEEL` for the buffer-history surface.
    ///
    /// * **Plain wheel** = vertical scroll through the lane list
    ///   (down-notch = show more buffers below).
    /// * **Ctrl+Wheel** = zoom the time axis about the pointer's
    ///   projected timestamp.
    /// * **Shift+Wheel** = horizontal pan (each notch shifts the
    ///   viewport by 10% of its current width).
    ///
    /// Returns `true` when consumed.
    pub(crate) fn try_buffer_history_wheel(
        &mut self,
        notches: f32,
        shift_held: bool,
        ctrl_held: bool,
        client_x: i32,
        client_y: i32,
    ) -> bool {
        if !self.focused_tab_is_buffer_history() {
            return false;
        }
        let tab_id = match self.tree.active_tab() {
            Some(t) if t.kind == TabKind::BufferHistory => t.id,
            _ => return false,
        };
        let rect = self.focused_buffer_history_panel_rect();
        // Wheel events outside the panel rect (over the tab strip or
        // status bar) fall through to their chrome handlers.
        if (client_x as f32) < rect.x
            || (client_x as f32) > rect.x + rect.w
            || (client_y as f32) < rect.y
            || (client_y as f32) > rect.y + rect.h
        {
            return false;
        }
        let draw = self.build_buffer_history_panel_draw(rect);
        let layout = compute_buffer_history_panel_layout(&draw);
        let visible_count = layout.visible_lane_capacity.max(1);
        // Pivot at pointer's projected timestamp.
        let strip = layout
            .lanes
            .first()
            .map(|l| l.strip_rect)
            .unwrap_or(draw.rect);
        let fraction = ((client_x as f32 - strip.x) / strip.w.max(1.0)).clamp(0.0, 1.0);
        let width = (draw.viewport_end_ms - draw.viewport_start_ms).max(1);
        let pivot_ms = draw.viewport_start_ms + (fraction as f64 * width as f64).round() as i64;
        let row_count = self
            .buffer_history_tabs
            .get(&tab_id)
            .map(|s| s.lanes.len())
            .unwrap_or(0);
        let Some(state) = self.buffer_history_tabs.get_mut(&tab_id) else {
            return false;
        };
        if ctrl_held {
            let factor = if notches > 0.0 {
                0.85_f32.powf(notches)
            } else {
                (1.0_f32 / 0.85).powf(-notches)
            };
            state.zoom_about(pivot_ms, factor);
        } else if shift_held {
            // Pan: each notch shifts by 10% of the current viewport
            // width — direction matches the wheel sign so up-wheel
            // moves forward in time.
            let width = state.viewport_width_ms();
            let delta = (-notches as f64 * width as f64 * 0.1).round() as i64;
            state.pan(delta);
        } else {
            // Vertical scroll. Up-wheel (positive notches) scrolls
            // *up* in the list (towards lane 0). Down-wheel scrolls
            // toward older buffers. One notch = three lanes for
            // brisk traversal of long lists.
            let step = 3_i32;
            let delta = (-notches.round() as i32) * step;
            let cur = state.scroll_lane_offset as i32;
            let max = row_count.saturating_sub(visible_count) as i32;
            state.scroll_lane_offset = (cur + delta).clamp(0, max).max(0) as usize;
        }
        // After mutating the viewport (zoom / pan / scroll) we must
        // re-evaluate the hover hit-test against the cursor's CURRENT
        // pixel position. Otherwise `hovered_lane` keeps pointing at
        // whatever lane WAS under the cursor before the wheel — the
        // preview band shows lane A while a subsequent click without
        // any mouse movement opens lane B (the row that scrolled
        // into A's pixel position). The hit-test runs against the
        // freshly-mutated state so the layout reflects the new
        // scroll / zoom / pan.
        let rect = self.focused_buffer_history_panel_rect();
        let draw = self.build_buffer_history_panel_draw(rect);
        let layout = compute_buffer_history_panel_layout(&draw);
        let hit = buffer_history_hit_test_lane(&layout, client_x as f32, client_y as f32);
        if let Some(state) = self.buffer_history_tabs.get_mut(&tab_id) {
            state.hovered_lane = hit;
        }
        self.request_repaint();
        true
    }

    /// Keyboard dispatch for the buffer-history surface. Returns
    /// `true` when the keystroke is claimed; the caller short-
    /// circuits the regular chord lookup.
    pub(crate) fn try_buffer_history_keystroke(&mut self, vk: u16) -> bool {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_LEFT, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT,
            VK_UP,
        };
        if !self.focused_tab_is_buffer_history() {
            return false;
        }
        let tab_id = match self.tree.active_tab() {
            Some(t) if t.kind == TabKind::BufferHistory => t.id,
            _ => return false,
        };
        let key = vk;
        if key == VK_ESCAPE.0 {
            // Close the history tab (returns the user to the
            // previous tab in the pane). The tab-close command name
            // is `tab.close`; route through the public path so MRU /
            // recently-closed handling matches a regular Ctrl+W.
            let _ = self.close_active_tab();
            return true;
        }
        if key == VK_RETURN.0 {
            self.confirm_buffer_history_tab_selection(tab_id);
            return true;
        }
        let delta: i32 = if key == VK_DOWN.0 || key == VK_RIGHT.0 {
            1
        } else if key == VK_UP.0 || key == VK_LEFT.0 {
            -1
        } else if key == VK_NEXT.0 {
            10
        } else if key == VK_PRIOR.0 {
            -10
        } else if key == VK_HOME.0 {
            i32::MIN
        } else if key == VK_END.0 {
            i32::MAX
        } else {
            return false;
        };
        if let Some(state) = self.buffer_history_tabs.get_mut(&tab_id) {
            state.step_lane(delta);
        }
        // Auto-scroll so the new selection stays visible. Compute
        // the current visible count by re-running the layout against
        // the focused pane's body rect (cheap — no D2D calls), then
        // adjust the scroll offset if `selected_lane` fell outside
        // the window.
        let rect = self.focused_buffer_history_panel_rect();
        let draw = self.build_buffer_history_panel_draw(rect);
        let layout = compute_buffer_history_panel_layout(&draw);
        let visible_count = layout.visible_lane_capacity.max(1);
        if let Some(state) = self.buffer_history_tabs.get_mut(&tab_id) {
            if let Some(sel) = state.selected_lane {
                if sel < state.scroll_lane_offset {
                    state.scroll_lane_offset = sel;
                } else if sel >= state.scroll_lane_offset + visible_count {
                    state.scroll_lane_offset = sel.saturating_sub(visible_count.saturating_sub(1));
                }
            }
        }
        self.request_repaint();
        true
    }

    /// WM_MOUSEMOVE hover handler for the buffer-history surface.
    /// Updates `hovered_lane` so the panel paints the hover chrome.
    /// Returns `true` when the move was claimed.
    pub(crate) fn on_buffer_history_mouse_move(&mut self, x: i32, y: i32, wparam: u32) -> bool {
        if !self.focused_tab_is_buffer_history() {
            return false;
        }
        let tab_id = match self.tree.active_tab() {
            Some(t) if t.kind == TabKind::BufferHistory => t.id,
            _ => return false,
        };
        const MK_LBUTTON: u32 = 0x0001;
        let lbutton_held = wparam & MK_LBUTTON != 0;
        // Drag-pan: when the user is holding the left button after a
        // miss-lane click, translate the viewport by the pointer's
        // horizontal delta. Runs even when the cursor leaves the
        // panel rect so the drag keeps tracking until WM_LBUTTONUP.
        if lbutton_held {
            let Some(state) = self.buffer_history_tabs.get_mut(&tab_id) else {
                return false;
            };
            if let Some(drag) = state.pan_drag {
                let delta_px = (x - drag.start_client_x) as f64;
                let span = (drag.start_viewport_end_ms - drag.start_viewport_start_ms) as f64;
                let strip_w = drag.strip_width_dip as f64;
                let delta_ms = -(delta_px * span / strip_w).round() as i64;
                state.viewport_start_ms = drag.start_viewport_start_ms + delta_ms;
                state.viewport_end_ms = drag.start_viewport_end_ms + delta_ms;
                self.request_repaint();
                return true;
            }
            return false;
        }
        let rect = self.focused_buffer_history_panel_rect();
        // Moves over the tab strip / status bar should not steal
        // hover state; fall through.
        if (x as f32) < rect.x
            || (x as f32) > rect.x + rect.w
            || (y as f32) < rect.y
            || (y as f32) > rect.y + rect.h
        {
            return false;
        }
        let draw = self.build_buffer_history_panel_draw(rect);
        let layout = compute_buffer_history_panel_layout(&draw);
        let hit = buffer_history_hit_test_lane(&layout, x as f32, y as f32);
        let mut changed = false;
        if let Some(state) = self.buffer_history_tabs.get_mut(&tab_id) {
            if state.hovered_lane != hit {
                state.hovered_lane = hit;
                changed = true;
            }
        }
        if changed {
            self.request_repaint();
        }
        true
    }
}
