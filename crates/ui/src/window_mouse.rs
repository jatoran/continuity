//! Mouse handlers (click / double-click / drag) for [`crate::Window`].

use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, VK_CONTROL, VK_MENU, VK_SHIFT,
};

use crate::window_input_modifiers::is_key_down;
use crate::window_mouse_hover::wall_clock_ms;

use crate::window_click_trace::{ClickLeftDownTrace, ClickStage};
use crate::Window;

impl Window {
    /// `WM_LBUTTONDOWN`: place the caret, or handle SegmentHit-driven
    /// link/checkbox interactions. Shift+click extends, Ctrl+click on a
    /// link opens the URL, a plain click on a checkbox segment toggles it.
    pub(crate) fn on_left_button_down(&mut self, x: i32, y: i32, key_state: u32) -> bool {
        self.cancel_scroll_inertia();
        let mut click_trace = ClickLeftDownTrace::new(x, y);
        // Cross-cutting overlay focus: when an overlay (find bar / palette
        // / picker) is active, clicks inside its panel are claimed by the
        // overlay so the editor caret never moves to a click that the user
        // intended for the overlay's input. For the dual-field find bar
        // this also flips `FindBar::focus` to whichever rect was clicked.
        if click_trace.measure(ClickStage::Overlay, || self.overlay_input_click(x, y)) {
            click_trace.claim("overlay");
            return true;
        }
        // Phase-I1: when the time-machine slider is open it owns clicks
        // inside its HUD band. Runs before any other click target so a
        // click on the slider doesn't bleed through into status-bar /
        // tab-strip / caret placement.
        if click_trace.measure(ClickStage::TimeMachine, || {
            self.try_time_machine_slider_left_down(x, y)
        }) {
            click_trace.claim("time_machine");
            return true;
        }
        if self.try_file_tree_left_down(x, y) {
            return true;
        }
        // Buffer-history tab: lane-click adopts that buffer as a new tab.
        if click_trace.measure(ClickStage::BufferHistory, || {
            self.try_buffer_history_left_down(x, y)
        }) {
            click_trace.claim("buffer_history");
            return true;
        }
        if click_trace.measure(ClickStage::StatusBar, || {
            self.try_status_bar_left_down(x, y)
        }) {
            click_trace.claim("status_bar");
            return true;
        }
        // D3: splitter drag begins before tab-strip/body routing so
        // the resize cursor and mouse-down target agree inside the
        // expanded splitter hit zone.
        if click_trace.measure(ClickStage::Splitter, || self.try_splitter_left_down(x, y)) {
            click_trace.claim("splitter");
            return true;
        }
        if click_trace.measure(ClickStage::TabStrip, || self.try_tab_strip_left_down(x, y)) {
            click_trace.claim("tab_strip");
            return true;
        }
        // Phase G4: a click on the search-active minimap strip jumps
        // the find bar to that match and never moves the caret. Must
        // run before pane-focus switch + caret placement so a click on
        // the strip in a focused pane doesn't double as a caret move.
        if click_trace.measure(ClickStage::SearchMinimap, || {
            self.try_search_minimap_left_down(x, y)
        }) {
            click_trace.claim("search_minimap");
            return true;
        }
        if click_trace.measure(ClickStage::Minimap, || self.try_minimap_left_down(x, y)) {
            click_trace.claim("minimap");
            return true;
        }
        // Phase F2: a click on an outline-sidebar row jumps the caret
        // to that heading line and scrolls it into view. Runs before
        // pane-body focus switch + caret placement so a click on the
        // strip doesn't double as a generic body-click.
        if click_trace.measure(ClickStage::Outline, || {
            self.try_outline_sidebar_left_down(x, y)
        }) {
            click_trace.claim("outline");
            return true;
        }
        // §H3: a click on a fold triangle toggles the fold on the
        // clicked line — caret stays put. Routed before caret
        // placement so a stray click on the triangle column doesn't
        // also move the caret to the triangle's row.
        if click_trace.measure(ClickStage::FoldTriangle, || {
            self.try_fold_triangle_left_down(x, y)
        }) {
            click_trace.claim("fold_triangle");
            return true;
        }
        // A click anywhere else in the line-number gutter moves the caret
        // to the start of the clicked line. The fold-toggle stage above
        // already consumed clicks on a collapse/expand toggle, so this
        // only fires for the rest of the gutter.
        if self.try_gutter_line_caret(x, y) {
            return true;
        }
        // Vertical scrollbar: thumb-grab starts a drag, track click
        // pages by one viewport. Routed before image hits / caret
        // placement so a click on the thumb (which lives at the right
        // edge of the body, possibly overlapping an inline-image
        // affordance) reaches the scrollbar.
        if click_trace.measure(ClickStage::Scrollbar, || self.try_scrollbar_left_down(x, y)) {
            click_trace.claim("scrollbar");
            return true;
        }
        // F5: a click on a collapsed-image affordance (thumbnail,
        // label, or chevron) flips the image's expand state. Hits
        // are recorded by the renderer's last paint pass in
        // pane-body-relative coordinates; translate from client
        // coords before testing.
        if click_trace.measure(ClickStage::ImageHit, || {
            let body = self.focused_body_rect();
            let x_pane = x as f32 - body.x;
            let y_pane = y as f32 - body.y;
            self.try_image_hit_at(x_pane, y_pane)
        }) {
            click_trace.claim("image_hit");
            return true;
        }
        click_trace.measure(ClickStage::CloseArm, || self.clear_unsaved_close_arm());
        // Fenced-code-block copy button: when the cursor sits inside
        // the live hover button rect, a click triggers the clipboard
        // copy and never reaches caret placement. Runs after overlay
        // / tab / status-bar / splitter (which own larger surfaces)
        // and before pane-focus switch + caret placement so a click
        // on the button leaves caret state untouched.
        if click_trace.measure(ClickStage::CodeCopy, || {
            self.try_code_copy_button_left_down(x, y)
        }) {
            click_trace.claim("code_copy");
            return true;
        }
        // Click in a non-focused pane body → switch focus to it first so
        // the caret lands in the clicked pane (rather than the previously
        // focused pane). `try_tab_strip_left_down` already handles the
        // tab-strip case; this covers everything below the strip.
        click_trace.measure(ClickStage::PaneFocus, || {
            self.try_pane_body_focus_switch(x as f32, y as f32);
        });
        // Phase 17.6 cleanup tail #5: route Ctrl+click / checkbox-click
        // through `SegmentHit` on the display map. Returns `true` when
        // the click was consumed by a link or checkbox segment so the
        // normal caret-placement path is skipped.
        if click_trace.measure(ClickStage::SegmentHit, || {
            let _s = crate::paint_trace::is_trace_enabled()
                .then(|| crate::paint_trace::EventScope::new("click_try_handle_segment_hit"));
            self.try_handle_segment_hit(x, y, key_state)
        }) {
            click_trace.claim("segment_hit");
            return true;
        }
        // Phase F — a mouse-down within the grab zone of a table column
        // boundary starts a live column-resize drag (own capture) rather
        // than placing the caret / selecting the cell.
        if self.try_table_col_resize_left_down(x, y) {
            return true;
        }
        let click_line = click_trace.measure(ClickStage::BufferPosition, || {
            let _s = crate::paint_trace::is_trace_enabled()
                .then(|| crate::paint_trace::EventScope::new("click_client_to_buffer_position"));
            self.client_to_buffer_position(x, y)
                .map(|p| p.line as i32)
                .unwrap_or(0)
        });
        let click_count = click_trace.measure(ClickStage::ClickState, || {
            let now_ms = wall_clock_ms();
            let click_count = self.mouse_state.register_click(now_ms, click_line);
            self.begin_selection_drag(x, y);
            click_count
        });
        let placement_handled = click_trace.measure(ClickStage::CaretPlacement, || {
            let _placement_scope = crate::paint_trace::is_trace_enabled().then(|| {
                crate::paint_trace::EventScope::with_detail(
                    "click_caret_placement",
                    format!("click_count={click_count}"),
                )
            });
            match click_count {
                2 => {
                    // Inside a table cell: double-click enters edit
                    // mode at the click position (no word selection).
                    // Outside a cell: standard double-click selects
                    // the word under the caret.
                    let in_cell = self.try_cell_hit_at_pixel(x, y).is_some();
                    let _ = self.place_caret_at_pixel(x, y, false);
                    if !in_cell {
                        let _ = Window::select_word(self);
                    }
                    true
                }
                3 => {
                    let _ = self.place_caret_at_pixel(x, y, false);
                    let _ = Window::select_line(self);
                    true
                }
                // Read live shift / alt state at click time instead of
                // trusting the cached `shift_held` (which only updates on
                // WM_KEYDOWN). Alt+Click drops an additional cursor at the
                // click position without disturbing existing selections;
                // Shift+Click extends the primary; bare click replaces.
                //
                // Single-click on a table cell selects the whole cell
                // content (Excel-style "selected" state). Delete/
                // Backspace then clear the cell, typing replaces it.
                // Double-click (handled above) enters edit mode at
                // the click point.
                _ => {
                    if is_key_down(VK_MENU.0) {
                        self.add_cursor_at_pixel(x, y)
                    } else if !is_key_down(VK_SHIFT.0) && self.try_select_cell_at_pixel(x, y) {
                        true
                    } else {
                        self.place_caret_at_pixel(x, y, is_key_down(VK_SHIFT.0))
                    }
                }
            }
        });
        if placement_handled {
            click_trace.claim("caret_placement");
        }
        placement_handled
    }

    /// `WM_LBUTTONDBLCLK`: select word at the click position. Win32 fires
    /// this in addition to `WM_LBUTTONDOWN` so we still bump the click count.
    /// Inside a table cell, double-click enters edit mode at the click
    /// position — no word selection — so the user can immediately type
    /// inside the cell instead of being stuck in the single-click
    /// "selected cell" state.
    pub(crate) fn on_left_button_dbl(&mut self, x: i32, y: i32) -> bool {
        if self.try_splitter_dbl_click(x, y) {
            return true;
        }
        if self.file_tree.is_visible() && x as f32 <= self.file_tree.visible_width_dip() {
            return true;
        }
        self.clear_unsaved_close_arm();
        let click_line = self
            .client_to_buffer_position(x, y)
            .map(|p| p.line as i32)
            .unwrap_or(0);
        let now_ms = wall_clock_ms();
        let _ = self.mouse_state.register_click(now_ms, click_line);
        self.begin_selection_drag(x, y);
        let in_cell = self.try_cell_hit_at_pixel(x, y).is_some();
        let placed = self.place_caret_at_pixel(x, y, false);
        if !in_cell {
            let _ = Window::select_word(self);
        }
        placed
    }

    /// `WM_MOUSEMOVE` while dragging: extend the selection to the cursor,
    /// or — when a tab is being dragged — just track the drag state
    /// (the actual drop is committed on `WM_LBUTTONUP`).
    pub(crate) fn on_mouse_move(&mut self, x: i32, y: i32, wparam: u32) -> bool {
        // Buffer-history tab: route the move to the lane hit-test so
        // the panel paints hover chrome on the row under the cursor,
        // or to the drag-pan handler when the user is holding the
        // left button after a miss-lane click.
        if self.on_buffer_history_mouse_move(x, y, wparam) {
            return true;
        }
        let invalidate_overlay = self.update_overlay_hover_from_pixel(x, y);
        if self.overlay_claims_pointer(x, y) {
            return invalidate_overlay;
        }
        // D6 — always update the tab-hover slot from the cursor's tab,
        // regardless of whether a mouse button is held (a drag still
        // updates the dwell time on the source tab if the user pauses
        // mid-drag, but the renderer can decide whether to paint based
        // on `tab_drag.is_some()`).
        let invalidate_hover = invalidate_overlay || self.update_tab_hover_from_pixel(x, y);
        // Track line/gutter hover even while a drag is in flight.
        let invalidate_line_hover = self.update_line_hover_from_pixel(x, y);
        let invalidate_footnote = if wparam & 0x0001 == 0 {
            self.update_footnote_hover_from_pixel(x, y)
        } else {
            self.clear_footnote_hover()
        };
        // Code-block copy-button hover: tracked while the left button
        // is up so it appears under a stationary cursor, suppressed
        // mid-drag (a selection drag's path may sweep through a fenced
        // block without the user intending to summon the button).
        let invalidate_code_copy = if wparam & 0x0001 == 0 {
            self.update_code_copy_hover_from_pixel(x, y)
        } else {
            self.clear_code_copy_hover()
        };
        let invalidate_hover = invalidate_hover
            || invalidate_line_hover
            || invalidate_footnote
            || invalidate_code_copy;
        // MK_LBUTTON = 1; only react when the left button is held (matches
        // the documented `wParam` flag bits).
        if wparam & 0x0001 == 0 {
            return invalidate_hover;
        }
        // Phase-I1: a slider drag claims WM_MOUSEMOVE while in flight.
        if self.try_time_machine_slider_mouse_move(x, y) {
            return true;
        }
        // Scrollbar drag claims WM_MOUSEMOVE while in flight — checked
        // before the `dragging` gate because the capture set on
        // `WM_LBUTTONDOWN` is what's keeping the messages flowing.
        if self.mouse_state.scrollbar_drag.is_some() {
            return self.try_scrollbar_drag_mouse_move(x, y);
        }
        if self.mouse_state.minimap_dragging {
            return self.try_minimap_drag_mouse_move(x, y);
        }
        // Phase F — a column-resize drag claims the move. Checked before
        // the `dragging` gate: the drag holds its own `SetCapture` but
        // does not set `mouse_state.dragging`.
        if self.mouse_state.table_col_drag.is_some() {
            return self.drag_table_col_resize(x);
        }
        if !self.mouse_state.dragging {
            return invalidate_hover;
        }
        if self.mouse_state.splitter_drag.is_some() {
            return self.drag_splitter(x, y);
        }
        if self.mouse_state.tab_drag.is_some() {
            // Mid-drag of a tab: don't commit a selection; the drop is
            // resolved on `WM_LBUTTONUP`. Recompute the live drop
            // resolution from (x, y) so the renderer can paint the
            // matching affordance (insertion bar, pane-body highlight,
            // foreign-window indicator, or tear-off ghost) at the
            // resolution the next mouse-up will actually commit.
            return self.on_tab_drag_mouse_move(x, y);
        }
        let placed = self.extend_drag_selection_at_pixel(x, y);
        self.update_mouse_drag_autoscroll_from_cursor(x, y);
        placed
    }

    /// `WM_LBUTTONUP`: commit any in-flight tab drag.
    ///
    /// Drop resolution, in priority order:
    /// 1. Pure click on the tab (cursor never left the strip) → no-op.
    /// 2. Cursor over a *different* pane inside this window → move tab.
    /// 3. Cursor over a sibling Continuity window on the *current*
    ///    virtual desktop → adopt the tab into that window.
    /// 4. Otherwise tear off into a fresh window — always — so a drop
    ///    on the desktop never silently "loses" the tab.
    pub(crate) fn on_left_button_up(&mut self, x: i32, y: i32) -> bool {
        let selection_drag_finished = self.finish_selection_drag_for_button_up();
        // Buffer-history tab: end an in-flight pan-drag. Runs first
        // so a drag release doesn't bleed into tab-drop / splitter
        // resolution paths below.
        if self.on_buffer_history_left_button_up() {
            let _ = (x, y);
            return true;
        }
        // Phase-I1: end an in-flight slider drag (releases capture).
        // Runs first so a slider drag doesn't accidentally trigger the
        // tab-drop / splitter-drop branches below.
        if self.try_time_machine_slider_left_up() {
            let _ = (x, y);
            return true;
        }
        if self.try_minimap_left_up() {
            let _ = (x, y);
            return true;
        }
        // Scrollbar drag terminates here — release capture, persist
        // the new scroll offset. Routed before splitter/tab branches
        // so a release inside a scrollbar drag doesn't accidentally
        // tear off a tab when the cursor wanders.
        if self.try_scrollbar_left_up() {
            let _ = (x, y);
            return true;
        }
        // Phase F — a column-resize drag commits its width to the table
        // directive and releases capture here. Routed before the
        // splitter / tab-drop branches so a release ending a resize drag
        // doesn't bleed into them.
        if self.finish_table_col_resize() {
            let _ = (x, y);
            return true;
        }
        // D3: splitter drag terminates here — release capture, persist.
        if self.mouse_state.splitter_drag.take().is_some() {
            unsafe {
                let _ = ReleaseCapture();
            }
            // P0.8.2 — splitter drag committed a wrap_width change for
            // the two adjacent panes. The drag itself fires per
            // WM_MOUSEMOVE tick (too noisy to prewarm), so the
            // prewarm happens once on the mouse-up. Only the focused
            // pane is dispatched; spectator-pane prewarm is deferred.
            let _ = self.try_dispatch_projection_worker_early("splitter_drag_end", "layout_change");
            self.request_state_save();
            let _ = (x, y);
            return true;
        }
        let Some(drag) = self.mouse_state.tab_drag.as_ref().cloned() else {
            return selection_drag_finished;
        };
        // Release the mouse capture set when the tab grab started.
        unsafe {
            let _ = ReleaseCapture();
        }
        let resolution = self.compute_tab_drop_resolution(&drag, x, y);
        // Notify any sibling window we were broadcasting hover to that
        // the drag is over so its preview affordance clears.
        self.broadcast_tab_drag_hover_leave(&drag);
        let elapsed = wall_clock_ms().saturating_sub(drag.start_ms);
        let foreign = match resolution {
            crate::mouse::TabDropResolution::ForeignWindow { hwnd_raw } => hwnd_raw as u64,
            _ => 0,
        };
        let slot = match resolution {
            crate::mouse::TabDropResolution::SourceStrip(i) => i.slot as i32,
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
        let ctrl_held = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
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
                if ctrl_held {
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
                if ctrl_held {
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
                let args = self
                    .client_dip_point_to_screen(x, y)
                    .map(|(drop_screen_x, drop_screen_y)| {
                        serde_json::json!({
                            "drop_screen_x": drop_screen_x,
                            "drop_screen_y": drop_screen_y,
                        })
                    })
                    .unwrap_or(serde_json::Value::Null);
                self.dispatch_command("window.tear_off_focused_tab", &args)
            }
        }
    }

    /// Click landed in a pane *body* (not its tab strip). Switch focus to
    /// the pane under the cursor before the caret painter runs.
    fn try_pane_body_focus_switch(&mut self, x: f32, y: f32) {
        let root = self.pane_root_rect();
        let Some((pane, _)) = crate::pane_layout::pane_at_point(&self.tree, root, x, y) else {
            return;
        };
        if pane == self.tree.focused {
            return;
        }
        self.switch_focus(pane);
    }
}

// Hit-test helpers live in `window_mouse_hit_test.rs`; hover helpers live
// in `window_mouse_hover.rs`.
