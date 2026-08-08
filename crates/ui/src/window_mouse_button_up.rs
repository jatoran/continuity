//! Mouse-button release routing for drags owned by [`crate::Window`].

use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, ReleaseCapture, VK_CONTROL};

use crate::window_mouse_hover::wall_clock_ms;
use crate::Window;

impl Window {
    /// Commit or cancel the active drag for `WM_LBUTTONUP`.
    pub(crate) fn on_left_button_up(&mut self, x: i32, y: i32) -> bool {
        if self.finish_file_tree_entry_drag(x, y) {
            return true;
        }
        let selection_drag_finished = self.finish_selection_drag_for_button_up();
        self.surface.pointer.multi_select_drag = false;
        if self.on_buffer_history_left_button_up()
            || self.try_time_machine_slider_left_up()
            || self.try_minimap_left_up()
            || self.try_scrollbar_left_up()
            || self.finish_table_col_resize()
            || self.finish_outline_resize()
            || self.finish_file_tree_resize()
        {
            return true;
        }
        if self.mouse_state.splitter_drag.take().is_some() {
            unsafe {
                let _ = ReleaseCapture();
            }
            let _ = self.try_dispatch_projection_worker_early("splitter_drag_end", "layout_change");
            self.request_state_save();
            return true;
        }
        let Some(drag) = self.mouse_state.tab_drag.as_ref().cloned() else {
            return selection_drag_finished;
        };
        unsafe {
            let _ = ReleaseCapture();
        }
        let resolution = self.compute_tab_drop_resolution(&drag, x, y);
        self.broadcast_tab_drag_hover_leave(&drag);
        let elapsed = wall_clock_ms().saturating_sub(drag.start_ms);
        let foreign = match resolution {
            crate::mouse::TabDropResolution::ForeignWindow { hwnd_raw } => hwnd_raw as u64,
            _ => 0,
        };
        let slot = match resolution {
            crate::mouse::TabDropResolution::SourceStrip(index) => index.slot as i32,
            _ => -1,
        };
        crate::paint_trace::log_event(
            "tab_drag",
            &format!(
                "state=drop target={target} slot={slot} foreign_hwnd={foreign} \
                 elapsed_ms_since_start={elapsed}",
                target = resolution.as_trace_str(),
            ),
        );
        self.clear_tab_drag_ghost();
        let is_control_held = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
        match resolution {
            crate::mouse::TabDropResolution::Cancel => false,
            crate::mouse::TabDropResolution::SourceStrip(target) => {
                if target.pane == drag.pane {
                    if let Some(group) = self.tree.groups.get_mut(&drag.pane) {
                        let new_index = target.slot.min(group.tabs.len().saturating_sub(1));
                        if group.reorder_tab(drag.tab, new_index) {
                            self.request_state_save();
                            return true;
                        }
                    }
                    return false;
                }
                if is_control_held {
                    let _ = self.clone_tab_to_pane(drag.tab, target.pane);
                } else {
                    let _ = self.move_tab_between_panes(drag.tab, drag.pane, target.pane);
                }
                true
            }
            crate::mouse::TabDropResolution::PaneBody {
                pane: target_pane, ..
            } => {
                if target_pane == drag.pane {
                    return false;
                }
                if is_control_held {
                    let _ = self.clone_tab_to_pane(drag.tab, target_pane);
                } else {
                    let _ = self.move_tab_between_panes(drag.tab, drag.pane, target_pane);
                }
                true
            }
            crate::mouse::TabDropResolution::ForeignWindow { .. } => {
                self.try_cross_window_tab_drop(drag, x, y)
            }
            crate::mouse::TabDropResolution::TearOff => {
                let arguments = self
                    .client_dip_point_to_screen(x, y)
                    .map(|(drop_screen_x, drop_screen_y)| {
                        serde_json::json!({
                            "drop_screen_x": drop_screen_x,
                            "drop_screen_y": drop_screen_y,
                        })
                    })
                    .unwrap_or(serde_json::Value::Null);
                self.dispatch_command("window.tear_off_focused_tab", &arguments)
            }
        }
    }
}
