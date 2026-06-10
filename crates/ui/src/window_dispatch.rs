//! Win32 message-pump entry point for [`crate::Window`].
//!
//! Extracted from `window.rs` so that file stays under the conventions
//! cap. Contains the static [`wndproc`] callback (registered as the
//! window-class handler) and the per-message [`Window::handle_message`]
//! dispatch that routes WM_* messages to the relevant `Window::on_*`
//! handler.
//!
//! Thread ownership: each `Window` is pinned to one UI thread; the
//! message pump only fires on that thread.

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DestroyWindow, GetWindowLongPtrW, PostQuitMessage, SetWindowLongPtrW,
    CREATESTRUCTW, GWLP_USERDATA, HTCLIENT, SIZE_MINIMIZED, WM_ACTIVATEAPP, WM_CAPTURECHANGED,
    WM_CHAR, WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY, WM_DPICHANGED, WM_DROPFILES, WM_ENTERSIZEMOVE,
    WM_ERASEBKGND, WM_EXITSIZEMOVE, WM_IME_COMPOSITION, WM_IME_ENDCOMPOSITION,
    WM_IME_STARTCOMPOSITION, WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_MOVE, WM_NCCREATE, WM_PAINT,
    WM_SETCURSOR, WM_SETFOCUS, WM_SETTINGCHANGE, WM_SIZE, WM_SYSCHAR, WM_SYSKEYDOWN, WM_SYSKEYUP,
    WM_TIMER,
};

use crate::window::Window;
use crate::window_helpers::lparam_to_xy;
use crate::window_theme::{lparam_is_immersive_color_set, read_system_dark};
use crate::window_timers::{
    CARET_BLINK_TIMER_ID, CODE_COPY_FEEDBACK_TIMER_ID, CONFIG_POLL_TIMER_ID,
    DECORATION_WATCHDOG_TIMER_ID, DISPLAY_PREWARM_TIMER_ID, FILE_IO_TIMER_ID,
    FOOTNOTE_HOVER_TIMER_ID, IMAGE_ANIMATION_TIMER_ID, METRICS_REPAINT_TIMER_ID, MOTION_TIMER_ID,
    MOUSE_DRAG_AUTOSCROLL_TIMER_ID, SCROLL_ANIM_TIMER_ID, STATE_SAVE_TIMER_ID,
    TAB_OVERLAY_HOLD_TIMER_ID, TRACE_SUMMARY_TIMER_ID,
};

/// Win32 window-class procedure: pulls the `Window *` back out of
/// `GWLP_USERDATA` and dispatches to [`Window::handle_message`]. Falls
/// through to `DefWindowProcW` when no `Window` is bound or the message
/// is unhandled.
pub(crate) unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = lparam.0 as *const CREATESTRUCTW;
        let ptr = unsafe { (*cs).lpCreateParams } as isize;
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr) };
    }
    let window_ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Window;
    if !window_ptr.is_null() {
        if let Some(result) = unsafe { (*window_ptr).handle_message(hwnd, msg, wparam, lparam) } {
            return result;
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

impl Window {
    /// Returns `Some(LRESULT)` when the message was handled, otherwise `None`
    /// to fall through to `DefWindowProcW`.
    pub(crate) unsafe fn handle_message(
        &mut self,
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Option<LRESULT> {
        let _wndproc_scope = if crate::paint_trace::is_trace_enabled() {
            let detail = self.trace_wndproc_detail(match msg {
                WM_TIMER => format!("timer_id={}", wparam.0),
                WM_PAINT => format!(
                    "reason={}",
                    crate::paint_trace_context::peek_invalidate_reason().unwrap_or("os_or_unknown")
                ),
                WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP => {
                    format!("vk={}", wparam.0)
                }
                WM_CHAR | WM_SYSCHAR => {
                    // lparam layout per Win32 docs:
                    //   bits  0..16  repeat count (auto-repeat coalesces)
                    //   bits 16..24  scan code
                    //   bit  30      previous key state (1 = was down ⇒ auto-repeat)
                    //   bit  31      transition state (1 = release)
                    let lp = lparam.0 as u32;
                    let repeat = lp & 0xFFFF;
                    let was_down = (lp >> 30) & 1;
                    format!(
                        "ch={} repeat={repeat} autorepeat={was_down} edits_since_paint={}",
                        wparam.0,
                        crate::paint_trace::edits_since_paint(),
                    )
                }
                WM_ACTIVATEAPP => format!("active={}", wparam.0),
                WM_ERASEBKGND => format!("hdc={}", wparam.0),
                WM_SIZE => {
                    let width = (lparam.0 as u32) & 0xffff;
                    let height = ((lparam.0 as u32) >> 16) & 0xffff;
                    format!("size_type={} width={} height={}", wparam.0, width, height)
                }
                WM_MOUSEMOVE => {
                    let (x, y) = lparam_to_xy(lparam);
                    format!(
                        "x={x} y={y} flags={} dragging={} splitter={} tab_drag={} scrollbar={}",
                        wparam.0,
                        self.mouse_state.dragging,
                        self.mouse_state.splitter_drag.is_some(),
                        self.mouse_state.tab_drag.is_some(),
                        self.mouse_state.scrollbar_drag.is_some(),
                    )
                }
                _ => format!("wparam={}", wparam.0),
            });
            Some(crate::paint_trace::WndprocScope::new(msg, &detail))
        } else {
            None
        };
        match msg {
            WM_PAINT => {
                if let Err(e) = self.on_paint(hwnd) {
                    eprintln!("paint error: {e}");
                }
                // Auto-arm the image-animation timer if a paint just
                // decoded an animated GIF. Cheap no-op when the cache
                // is all-static or the timer is already running.
                self.ensure_image_animation_timer();
                Some(LRESULT(0))
            }
            WM_ERASEBKGND => {
                // The D2D/DXGI paint path clears the whole render target.
                // During modal live resize Windows still asks the HWND to
                // erase between non-client sizing steps; report handled so
                // DefWindowProc never exposes a transient class/default
                // background along the right/bottom edge.
                Some(LRESULT(1))
            }
            WM_SIZE => {
                let _ = lparam;
                // β — track minimized state for the background-window
                // vsync drop. SIZE_MINIMIZED == 1 fires when the window
                // is iconified; any other value (RESTORED / MAXIMIZED /
                // MAXSHOW) means the surface is visible again.
                self.is_window_minimized = wparam.0 as u32 == SIZE_MINIMIZED;
                if self.is_applying_dpi_change {
                    return None;
                }
                let old_client_width = self.client_width;
                let old_client_height = self.client_height;
                self.refresh_client_size(hwnd);
                if self.inited {
                    // Phase 17: user-driven resizes mutate the persisted
                    // placement_blob. Debounce so dragging a corner emits
                    // one save when the user lets go, not one per frame.
                    self.request_state_save();
                    // P0.8.2 — programmatic resizes (snap-to-side,
                    // restore-from-minimized, DPI restore) commit a
                    // wrap_width change outside a sizing loop. Prewarm
                    // the focused pane and every spectator pane so the
                    // next paint can hit a worker result on each (a
                    // window resize shifts wrap_width for every live
                    // pane, not just the focused one). Live-resize
                    // ticks are excluded — they fire per pixel and the
                    // WM_EXITSIZEMOVE hook covers the committed final
                    // size. Minimized windows are also skipped — no
                    // visible geometry to prewarm. The brief's option
                    // (a) "fire on every WM_SIZE" was tried and
                    // reverted: trace `20260522-153911` showed the
                    // worker channel saturating (4 rejected submissions
                    // at depth=64), the old placeholder strategy going
                    // blank across the drag (UX regression — spectators
                    // appeared empty while dragging), and the rejected
                    // submissions still leaving a 1.28 s cold walker
                    // at the final tick. The original deliberate skip
                    // is the right trade-off here.
                    if !self.is_live_resizing && !self.is_window_minimized {
                        self.try_dispatch_layout_projection_worker_for_live_panes("wm_size");
                    }
                }
                if self.is_live_resizing && self.inited && !self.is_window_minimized {
                    self.handle_live_resize_update_window(
                        hwnd,
                        old_client_width,
                        old_client_height,
                    );
                }
                Some(LRESULT(0))
            }
            WM_ENTERSIZEMOVE => {
                // Win32 modal sizing/move loop starts here. Snapshot
                // the caret anchor once so per-tick anchor work in
                // `refresh_client_size` can be skipped; the snapshot
                // is restored once at WM_EXITSIZEMOVE against the
                // final projection.
                self.is_live_resizing = true;
                self.resize_changed = false;
                self.resize_anchor = self.capture_caret_anchor();
                None
            }
            WM_EXITSIZEMOVE => {
                self.is_live_resizing = false;
                // Belt-and-suspenders: ensure the final size is
                // captured (some sizing loops can end without a
                // trailing WM_SIZE delta on the very last edge).
                self.refresh_client_size(hwnd);
                if self.resize_changed {
                    if let Some(anchor) = self.resize_anchor.take() {
                        self.restore_caret_anchor(anchor);
                    }
                    // One last invalidate so the corrected scroll
                    // position paints under the final viewport.
                    self.invalidate(hwnd);
                    // P0.8.2 — end of a Win32 modal sizing loop. The
                    // per-tick WM_SIZE deltas are intentionally not
                    // prewarmed (they'd spam the worker queue with
                    // stamps the user never paints at); the final
                    // committed size is prewarmed here so the next
                    // WM_PAINT can hit a worker result. Fans out
                    // across the focused pane and every spectator
                    // pane — a window resize shifts wrap_width for
                    // every live pane.
                    self.try_dispatch_layout_projection_worker_for_live_panes("exit_size_move");
                } else {
                    self.resize_anchor = None;
                }
                self.resize_changed = false;
                None
            }
            WM_ACTIVATEAPP => {
                // β — app focus toggles drive the 30 Hz unfocused
                // frame-skip in on_paint. wparam is non-zero when the
                // window is becoming the active app window. Body lives
                // in `window_focus.rs`.
                self.on_activate_app(hwnd, wparam.0 != 0);
                None
            }
            // Keyboard-focus edges (fire on intra-process window
            // switches that WM_ACTIVATEAPP never sees). Bodies live in
            // `window_focus.rs`.
            WM_SETFOCUS => {
                self.on_set_focus(hwnd);
                None
            }
            WM_KILLFOCUS => {
                self.on_kill_focus(hwnd);
                None
            }
            WM_MOVE => {
                // Window dragged to a new screen position. Like WM_SIZE this
                // mutates placement_blob; also covers monitor handoffs on
                // multi-monitor setups. Debounce coalesces a drag into one
                // save. WM_MOVE does NOT fire for virtual-desktop migrations
                // (VD switches are transparent at the HWND level) — VD
                // capture happens at save-time via current_desktop_guid, so
                // saving on move is enough to re-capture once the user does
                // anything that triggers a subsequent save tick.
                if self.inited {
                    self.request_state_save();
                }
                None
            }
            WM_CHAR => {
                // Suppress WM_CHAR while an IME composition is in flight
                // — committed text is delivered via WM_IME_COMPOSITION's
                // GCS_RESULTSTR path instead, and double-insertion would
                // otherwise occur.
                if self.ime_state.composing {
                    return Some(LRESULT(0));
                }
                if self.on_char(wparam.0 as u32) {
                    self.invalidate(hwnd);
                }
                Some(LRESULT(0))
            }
            WM_IME_STARTCOMPOSITION => {
                self.on_ime_start_composition();
                Some(LRESULT(0))
            }
            WM_IME_COMPOSITION => {
                if self.on_ime_composition(hwnd, lparam.0) {
                    self.invalidate(hwnd);
                }
                // Returning DefWindowProcW keeps the IME UI working
                // (candidate windows etc.).
                None
            }
            WM_IME_ENDCOMPOSITION => {
                self.on_ime_end_composition();
                self.invalidate(hwnd);
                None
            }
            WM_CONTEXTMENU => self.on_context_menu(hwnd, lparam.0).then_some(LRESULT(0)),
            WM_KEYDOWN => {
                let vk = wparam.0 as u16;
                if self.on_keydown(vk) {
                    self.invalidate(hwnd);
                }
                Some(LRESULT(0))
            }
            WM_KEYUP => {
                let vk = wparam.0 as u16;
                if self.on_keyup(vk) {
                    self.invalidate(hwnd);
                }
                Some(LRESULT(0))
            }
            // Alt-modified keys arrive as WM_SYSKEYDOWN, not WM_KEYDOWN —
            // route them through the keymap so `Alt+...` / `Ctrl+Alt+...`
            // chords dispatch. Only swallow the message when we actually
            // dispatched a command; unhandled syskeys (Alt+F4, plain Alt
            // menu-activate, etc.) must fall through to DefWindowProcW.
            WM_SYSKEYDOWN => {
                let vk = wparam.0 as u16;
                if self.on_keydown(vk) {
                    self.invalidate(hwnd);
                    Some(LRESULT(0))
                } else {
                    None
                }
            }
            WM_SYSKEYUP => {
                let vk = wparam.0 as u16;
                if self.on_keyup(vk) {
                    self.invalidate(hwnd);
                    Some(LRESULT(0))
                } else {
                    None
                }
            }
            // Suppress the Windows "menu item not found" beep that
            // `DefWindowProc` plays for every unhandled `WM_SYSCHAR`.
            // `TranslateMessage` synthesizes this message for any Alt+key
            // press regardless of how `WM_SYSKEYDOWN` was handled, and
            // this app has no menu accelerators — every Alt-bearing
            // chord routes through the keymap above, so the synthesized
            // SYSCHAR is always garbage we want to drop on the floor.
            WM_SYSCHAR => Some(LRESULT(0)),
            WM_LBUTTONDOWN => {
                let (x, y) = lparam_to_xy(lparam);
                let (x, y) = self.physical_point_to_dip(x, y);
                if self.on_left_button_down(x, y, wparam.0 as u32) {
                    self.invalidate(hwnd);
                }
                Some(LRESULT(0))
            }
            WM_LBUTTONDBLCLK => {
                let (x, y) = lparam_to_xy(lparam);
                let (x, y) = self.physical_point_to_dip(x, y);
                if self.on_left_button_dbl(x, y) {
                    self.invalidate(hwnd);
                }
                Some(LRESULT(0))
            }
            WM_LBUTTONUP => {
                let (x, y) = lparam_to_xy(lparam);
                let (x, y) = self.physical_point_to_dip(x, y);
                if self.on_left_button_up(x, y) {
                    self.invalidate(hwnd);
                }
                self.mouse_state.dragging = false;
                self.mouse_state.tab_drag = None;
                self.mouse_state.splitter_drag = None;
                Some(LRESULT(0))
            }
            WM_MOUSEMOVE => {
                let (x, y) = lparam_to_xy(lparam);
                let (x, y) = self.physical_point_to_dip(x, y);
                self.ensure_mouse_leave_tracking(hwnd);
                if self.on_mouse_move(x, y, wparam.0 as u32) {
                    self.invalidate(hwnd);
                }
                Some(LRESULT(0))
            }
            WM_MOUSELEAVE => {
                if self.on_mouse_leave() {
                    self.invalidate(hwnd);
                }
                Some(LRESULT(0))
            }
            WM_CAPTURECHANGED => {
                self.on_capture_changed();
                Some(LRESULT(0))
            }
            WM_MBUTTONDOWN => {
                let (x, y) = lparam_to_xy(lparam);
                let (x, y) = self.physical_point_to_dip(x, y);
                if self.on_middle_button_down(x, y) {
                    self.invalidate(hwnd);
                }
                Some(LRESULT(0))
            }
            WM_SETCURSOR => {
                // Only own the cursor over our own client area. Title bar,
                // borders, and the like stay with the system's defaults.
                let hit = (lparam.0 & 0xFFFF) as u16;
                if hit != HTCLIENT as u16 {
                    return None;
                }
                if self.on_set_cursor(hwnd) {
                    Some(LRESULT(1))
                } else {
                    None
                }
            }
            WM_MOUSEWHEEL => {
                let delta_raw = ((wparam.0 >> 16) & 0xFFFF) as i16;
                let key_state = (wparam.0 & 0xFFFF) as u32;
                // `lParam` carries the cursor position in *screen*
                // coordinates for WM_MOUSEWHEEL (unlike WM_MOUSEMOVE,
                // which is already client-local). Convert so the
                // time-machine slider can hover-test against its
                // client-rect HUD band.
                let screen_x = (lparam.0 & 0xFFFF) as i16;
                let screen_y = ((lparam.0 >> 16) & 0xFFFF) as i16;
                let mut pt = POINT {
                    x: i32::from(screen_x),
                    y: i32::from(screen_y),
                };
                let (client_x, client_y) = if unsafe { ScreenToClient(hwnd, &mut pt) }.as_bool() {
                    (pt.x, pt.y)
                } else {
                    (i32::from(screen_x), i32::from(screen_y))
                };
                let (client_x, client_y) = self.physical_point_to_dip(client_x, client_y);
                self.clear_unsaved_close_arm();
                if self.on_mouse_wheel(hwnd, delta_raw as f32, key_state, client_x, client_y) {
                    self.invalidate(hwnd);
                }
                Some(LRESULT(0))
            }
            WM_DROPFILES => {
                self.on_drop_files(wparam.0 as isize);
                Some(LRESULT(0))
            }
            WM_TIMER => {
                if wparam.0 == SCROLL_ANIM_TIMER_ID {
                    self.on_scroll_anim_tick(hwnd);
                } else if wparam.0 == CARET_BLINK_TIMER_ID {
                    self.on_caret_blink_tick(hwnd);
                } else if wparam.0 == CONFIG_POLL_TIMER_ID {
                    self.on_config_poll_tick(hwnd);
                } else if wparam.0 == FILE_IO_TIMER_ID {
                    self.on_file_io_tick(hwnd);
                } else if wparam.0 == STATE_SAVE_TIMER_ID {
                    self.on_state_save_tick(hwnd);
                } else if wparam.0 == METRICS_REPAINT_TIMER_ID {
                    self.on_metrics_repaint_tick(hwnd);
                } else if wparam.0 == TAB_OVERLAY_HOLD_TIMER_ID {
                    self.on_tab_overlay_hold_tick(hwnd);
                    self.invalidate(hwnd);
                } else if wparam.0 == DISPLAY_PREWARM_TIMER_ID {
                    self.on_display_prewarm_tick(hwnd);
                } else if wparam.0 == DECORATION_WATCHDOG_TIMER_ID {
                    self.on_decoration_watchdog_tick(hwnd);
                } else if wparam.0 == MOTION_TIMER_ID {
                    self.on_motion_tick(hwnd);
                } else if wparam.0 == FOOTNOTE_HOVER_TIMER_ID {
                    self.on_footnote_hover_timer(hwnd);
                    self.invalidate(hwnd);
                } else if wparam.0 == CODE_COPY_FEEDBACK_TIMER_ID {
                    self.on_code_copy_feedback_timer(hwnd);
                    self.invalidate(hwnd);
                } else if wparam.0 == IMAGE_ANIMATION_TIMER_ID {
                    self.on_image_animation_tick(hwnd);
                } else if wparam.0 == TRACE_SUMMARY_TIMER_ID {
                    crate::paint_trace_summary::tick();
                    crate::memory_trace::emit_snapshot(self);
                } else if wparam.0 == MOUSE_DRAG_AUTOSCROLL_TIMER_ID {
                    self.on_mouse_drag_autoscroll_timer(hwnd);
                }
                Some(LRESULT(0))
            }
            WM_SETTINGCHANGE => {
                if lparam_is_immersive_color_set(lparam) {
                    let dark = read_system_dark();
                    if self.active_theme.set_system_dark(dark) {
                        self.invalidate_with_reason(hwnd, "theme_apply");
                    }
                }
                None
            }
            WM_DPICHANGED => {
                if let Err(e) = self.handle_dpi_changed(hwnd, wparam, lparam) {
                    eprintln!("dpi change error: {e}");
                }
                Some(LRESULT(0))
            }
            WM_CLOSE => {
                // The user clicked the title-bar X, hit Alt+F4, or the
                // pane-collapse path posted WM_CLOSE because the last
                // tab went away. Prompt if any tab has unsaved typing;
                // returning `LRESULT(0)` without calling `DestroyWindow`
                // cancels the close. On confirm we explicitly destroy
                // so `WM_DESTROY` still fires the placement-save +
                // cleanup arm below.
                if !self.confirm_close_window() {
                    return Some(LRESULT(0));
                }
                let _ = unsafe { DestroyWindow(hwnd) };
                Some(LRESULT(0))
            }
            WM_DESTROY => {
                // Phase 16.5: snapshot stays per-window; tombstone-vs-
                // preserve lives in the registry's Closed-event handler.
                self.save_window_placement_state();
                // Phase I2: flush any in-flight metrics delta before the
                // persist client is dropped so the day's row reflects the
                // session's final keystrokes.
                let now_ms = self.now_ms();
                self.flush_metrics_now(now_ms);
                self.stop_metrics_repaint_timer();
                crate::window_registry::unregister(hwnd);
                unsafe { PostQuitMessage(0) };
                Some(LRESULT(0))
            }
            // Phase 17.6 cross-window tab-drop signal.
            m if m == windows::Win32::UI::WindowsAndMessaging::WM_USER + 1 => {
                if self.drain_cross_window_adoptions(hwnd) {
                    self.invalidate(hwnd);
                }
                Some(LRESULT(0))
            }
            // Cross-window tab-drag hover broadcast — a sibling
            // Continuity window's drag is currently hovering this
            // window so we paint a matching drop affordance.
            m if m == crate::window_tab_drag::tab_drag_hover_message_id() && m != 0 => {
                if self.on_foreign_tab_drag_hover(wparam, lparam) {
                    self.invalidate(hwnd);
                }
                Some(LRESULT(0))
            }
            // δ.3 test probe — `view.scroll_y_dip * 1000` as LRESULT.
            // Resolution of one milli-DIP is enough for the caret-anchor
            // integration tests; floats are not Send-portable through
            // LRESULT, so the harness scales back on receipt.
            m if m == windows::Win32::UI::WindowsAndMessaging::WM_USER + 2 => {
                let scaled = (self.view.scroll_y_dip * 1000.0).round() as isize;
                Some(LRESULT(scaled))
            }
            // Spectator cache probe — `(hits << 32) | misses` packed
            // into the LRESULT. Used by the split-pane regression to
            // verify the per-pane projection cache hits on the second
            // paint when typing in a small focused pane against a
            // large non-focused buffer.
            m if m == windows::Win32::UI::WindowsAndMessaging::WM_USER + 3 => {
                let cache = self.spectator_frame_cache.borrow();
                let hits = cache.hits() as isize;
                let misses = cache.misses() as isize;
                let packed = ((hits & 0xFFFF_FFFF) << 32) | (misses & 0xFFFF_FFFF);
                Some(LRESULT(packed))
            }
            // Caret-anchor capture probe. Used by layout-shortcut
            // regressions to assert unchanged focused-pane geometry
            // does not enter the expensive anchor path.
            m if m == windows::Win32::UI::WindowsAndMessaging::WM_USER + 4 => Some(LRESULT(
                self.caret_anchor_capture_count.get().min(isize::MAX as u64) as isize,
            )),
            // Current window DPI probe for integration tests.
            m if m == windows::Win32::UI::WindowsAndMessaging::WM_USER + 5 => {
                Some(LRESULT(self.window_dpi as isize))
            }
            // Font-state probe: returns the active key bits.
            m if m == windows::Win32::UI::WindowsAndMessaging::WM_USER + 6 => {
                Some(LRESULT(self.font_state.0 as isize))
            }
            // Layout-cache probe: WPARAM carries a FontStateId bit pattern;
            // return how many cached layouts still use that key.
            m if m == windows::Win32::UI::WindowsAndMessaging::WM_USER + 7 => {
                let font_state = continuity_layout::FontStateId(wparam.0 as u64);
                Some(LRESULT(
                    self.cache.entry_count_for_font_state(font_state) as isize
                ))
            }
            // Caret-line screen-y probe, returned in milli-DIPs.
            m if m == windows::Win32::UI::WindowsAndMessaging::WM_USER + 8 => {
                let scaled = self
                    .current_primary_caret_screen_y_dip()
                    .map(|(_, screen_y)| (screen_y * 1000.0).round() as isize)
                    .unwrap_or(isize::MIN);
                Some(LRESULT(scaled))
            }
            _ => None,
        }
    }
}
