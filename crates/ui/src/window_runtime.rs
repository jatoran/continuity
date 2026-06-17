//! Runtime helpers for timers, scrolling, and snapshots.
//!
//! Split from `window.rs` so the message-dispatch module stays under the
//! file-length convention. All methods run on the owning UI thread.

use continuity_core::EditorSnapshot;
use continuity_layout::{MAX_ZOOM, MIN_ZOOM};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer, WHEEL_DELTA};

use crate::window::{Window, CARET_BLINK_TIMER_ID, SCROLL_ANIM_TIMER_ID, SCROLL_ANIM_TIMER_MS};

impl Window {
    pub(crate) fn invalidate(&self, hwnd: HWND) {
        self.invalidate_with_reason(hwnd, "invalidate_rect");
    }

    /// Invalidate with a call-site reason for the trace. The reason
    /// surfaces on the next `paint:TOTAL` as `reason=<reason>` so a
    /// trace consumer can see why a given paint frame fired. Reasons
    /// in use: `invalidate_rect`, `scroll_anim`,
    /// `decoration_delivered`, `prewarm_tick`, `caret_blink`,
    /// `theme_apply`, `external_invalidate`, `dpi_changed`, plus
    /// internal animation/status reasons.
    pub(crate) fn invalidate_with_reason(&self, hwnd: HWND, reason: &'static str) {
        crate::paint_trace::note_invalidate_request_with_reason(reason);
        if crate::paint_trace::is_trace_enabled() {
            crate::paint_trace::log_event(
                "invalidate_request",
                &format!(
                    "hwnd={} client={}x{} focused={} minimized={} edits_since_paint={} \
                     reason={reason}",
                    hwnd.0 as usize,
                    self.client_width,
                    self.client_height,
                    self.is_window_focused,
                    self.is_window_minimized,
                    crate::paint_trace::edits_since_paint(),
                ),
            );
        }
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(hwnd), None, false);
        }
    }

    /// Cancel any running scroll animation.
    pub(crate) fn stop_scroll_anim(&mut self, hwnd: HWND) {
        self.cancel_scroll_inertia();
        if self.scroll_anim_active {
            unsafe {
                let _ = KillTimer(Some(hwnd), SCROLL_ANIM_TIMER_ID);
            }
            self.scroll_anim_active = false;
        }
    }

    /// Start the smooth-scroll timer if needed.
    pub(crate) fn start_scroll_anim(&mut self, hwnd: HWND) {
        if !self.scroll_anim_active {
            unsafe {
                let _ = SetTimer(Some(hwnd), SCROLL_ANIM_TIMER_ID, SCROLL_ANIM_TIMER_MS, None);
            }
            self.scroll_anim_active = true;
        }
    }

    /// Start the caret-blink timer.
    pub(crate) fn start_caret_blink(&mut self, hwnd: HWND) {
        let period = self.view_options.caret_blink_ms;
        if period == 0 {
            self.caret_blink_visible = true;
            return;
        }
        if !self.caret_blink_active {
            unsafe {
                let _ = SetTimer(Some(hwnd), CARET_BLINK_TIMER_ID, period / 2, None);
            }
            self.caret_blink_active = true;
        }
    }

    /// Mark the user as having just produced input so the caret stays
    /// visible while typing.
    pub(crate) fn note_input_now(&mut self) {
        self.last_input_tick = unsafe { GetTickCount64() };
        self.cancel_active_display_prewarm();
        if !self.caret_blink_visible {
            self.caret_blink_visible = true;
            self.invalidate(self.hwnd);
        }
    }

    pub(crate) fn on_caret_blink_tick(&mut self, hwnd: HWND) {
        // Phase B6/B7: piggy-back glow + tween eviction on the blink
        // tick so we don't need extra timers.
        self.evict_expired_jump_glow();
        self.evict_expired_caret_tween();
        let now = unsafe { GetTickCount64() };
        if blink_paused(
            self.view_options.caret_blink_on_typing_pause,
            self.view_options.caret_typing_pause_ms,
            self.view_options.caret_long_idle_ms,
            self.last_input_tick,
            now,
        ) {
            if !self.caret_blink_visible {
                self.caret_blink_visible = true;
                self.invalidate_with_reason(hwnd, "caret_blink");
            }
            return;
        }
        self.caret_blink_visible = !self.caret_blink_visible;
        self.invalidate_with_reason(hwnd, "caret_blink");
    }

    pub(crate) fn on_scroll_anim_tick(&mut self, hwnd: HWND) {
        let now_ms = unsafe { GetTickCount64() };
        let inertia_tick = self.tick_scroll_inertia();
        let moved = inertia_tick.moved || self.view.tick(now_ms);
        if moved {
            self.invalidate_with_reason(hwnd, "scroll_anim");
        }
        if !self.view.animating() && !self.is_scroll_inertia_active() {
            self.stop_scroll_anim(hwnd);
        }
    }

    pub(crate) fn on_mouse_wheel(
        &mut self,
        hwnd: HWND,
        delta: f32,
        key_state: u32,
        client_x: i32,
        client_y: i32,
    ) -> bool {
        const MK_CONTROL: u32 = 0x0008;
        const MK_SHIFT: u32 = 0x0004;
        let notches = delta / WHEEL_DELTA as f32;
        // Phase-I1: when the time-machine slider is open and the
        // cursor is over the HUD band, each wheel notch steps the
        // preview revision instead of scrolling the buffer.
        if self.try_time_machine_slider_wheel(client_x, client_y, notches) {
            return true;
        }
        // Buffer-history tab: plain wheel = vertical lane scroll,
        // Ctrl+Wheel = zoom about pointer, Shift+Wheel = horizontal
        // pan. Runs before the standard Ctrl-zoom / line-scroll
        // path because the history tab is a non-text surface and
        // shouldn't scroll-by-line.
        let shift_held = (key_state & MK_SHIFT) != 0;
        let ctrl_held = (key_state & MK_CONTROL) != 0;
        if !self.client_point_inside_window(client_x, client_y) {
            return false;
        }
        if self.should_suppress_wheel_for_active_drag() {
            return false;
        }
        if self.try_file_tree_mouse_wheel(client_x, client_y, notches) {
            return true;
        }
        if self.try_buffer_history_wheel(notches, shift_held, ctrl_held, client_x, client_y) {
            return true;
        }
        if self.try_palette_mouse_wheel(client_x, client_y, notches) {
            return true;
        }
        if self.overlay_claims_pointer(client_x, client_y) {
            return false;
        }
        if self.try_outline_sidebar_mouse_wheel(client_x, client_y, notches) {
            return true;
        }
        if (key_state & MK_CONTROL) != 0 {
            let factor =
                compute_ctrl_wheel_zoom_factor(notches, self.settings_projections.zoom_step_pct);
            // Funnel through the global text-scale mutator so wheel zoom
            // is global + persisted just like the zoom commands: it
            // applies to every pane (anchored on the focused caret line)
            // and writes `[editor].text_scale` back, fanning out to all
            // windows. The settings write-back is idempotent, so a notch
            // that lands on the clamp boundary does not thrash the file.
            let new_scale = (self.view.font_size_scale * factor).clamp(MIN_ZOOM, MAX_ZOOM);
            self.apply_global_text_scale(new_scale);
            return true;
        }
        // Item 8(d) — Shift+wheel over an overflowing tab strip scrolls the
        // strip horizontally instead of the buffer body.
        if shift_held && self.try_tab_strip_wheel_scroll(client_x, client_y, notches) {
            return true;
        }
        let Some(target_pane) = self.wheel_scroll_target_at(client_x, client_y) else {
            return false;
        };
        self.apply_wheel_scroll(hwnd, target_pane, notches)
    }

    fn client_point_inside_window(&self, x: i32, y: i32) -> bool {
        let x = x as f32;
        let y = y as f32;
        x >= 0.0 && y >= 0.0 && x < self.client_width_dip() && y < self.client_height_dip()
    }

    fn should_suppress_wheel_for_active_drag(&self) -> bool {
        self.mouse_state.dragging
            || self.mouse_state.tab_drag.is_some()
            || self.mouse_state.splitter_drag.is_some()
            || self.mouse_state.scrollbar_drag.is_some()
    }

    /// Estimated total content height in DIPs. Uses the last painted
    /// projection's exact display-row count when available so soft-wrap,
    /// folds, and reserved image rows clamp against real content rather
    /// than source lines or scrollbar estimates.
    pub(crate) fn estimated_content_height(&self) -> f32 {
        let line_height = self.effective_line_height();
        if let Some((_, fd)) = self.last_painted_frame_display.as_ref() {
            let display_rows = fd.display_line_count().max(1) as f32;
            return display_rows * line_height;
        }
        let snap = match self.editor.snapshot(self.buffer_id) {
            Some(s) => s,
            None => return 0.0,
        };
        let lines = snap.rope_snapshot().rope().len_lines().max(1) as f32;
        lines * line_height
    }

    /// HWND accessor used by view-command implementations. Public
    /// (rather than `pub(crate)`) so the §C1 Win32 test harness in
    /// `continuity_test_support` can dispatch messages off-thread
    /// via `SendMessageW` / `PostMessageW`.
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub(crate) fn current_snapshot(&self) -> Option<EditorSnapshot> {
        self.editor.snapshot(self.buffer_id)
    }
}

fn compute_ctrl_wheel_zoom_factor(notches: f32, zoom_step_pct: u32) -> f32 {
    let step = 1.0 + zoom_step_pct.clamp(1, 100) as f32 / 100.0;
    if notches > 0.0 {
        step.powf(notches)
    } else {
        (1.0 / step).powf(-notches)
    }
}

/// Pure decision: should the caret-blink tick stay solid?
///
/// Two windows are suspended; the visible-blink window lies between
/// them:
///   * `[0, pause_ms)` after last input — Phase B5 typing-burst pause.
///   * `[long_idle_ms, ∞)` after last input — α.3 long-idle suspend.
///
/// Both windows respect `enabled`. Either `pause_ms == 0` or
/// `long_idle_ms == 0` disables that individual window. Free function
/// so it can be unit tested without a real Window.
fn blink_paused(
    enabled: bool,
    pause_ms: u32,
    long_idle_ms: u32,
    last_input: u64,
    now: u64,
) -> bool {
    if !enabled {
        return false;
    }
    let elapsed = now.saturating_sub(last_input);
    if pause_ms > 0 && elapsed < u64::from(pause_ms) {
        return true;
    }
    if long_idle_ms > 0 && elapsed >= u64::from(long_idle_ms) {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{blink_paused, compute_ctrl_wheel_zoom_factor};

    #[test]
    fn ctrl_wheel_zoom_factor_uses_configured_step() {
        let factor = compute_ctrl_wheel_zoom_factor(2.0, 25);
        assert!((factor - 1.5625).abs() < f32::EPSILON);
    }

    #[test]
    fn ctrl_wheel_zoom_factor_handles_zoom_out() {
        let factor = compute_ctrl_wheel_zoom_factor(-1.0, 25);
        assert!((factor - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn pause_active_within_window() {
        assert!(blink_paused(true, 400, 6_000, 1000, 1100));
    }

    #[test]
    fn pause_releases_after_window() {
        assert!(!blink_paused(true, 400, 6_000, 1000, 1500));
    }

    #[test]
    fn pause_at_exact_boundary_releases() {
        assert!(!blink_paused(true, 400, 6_000, 1000, 1400));
    }

    #[test]
    fn opt_out_never_pauses() {
        assert!(!blink_paused(false, 400, 6_000, 1000, 1000));
        assert!(!blink_paused(false, 10_000, 6_000, 1000, 1001));
        // α.3: opt-out also disables the long-idle suspend.
        assert!(!blink_paused(false, 400, 6_000, 1000, 999_999));
    }

    #[test]
    fn pause_zero_disables_typing_window() {
        assert!(!blink_paused(true, 0, 6_000, 1000, 1000));
        // Long-idle still kicks in when its own window is reached.
        assert!(blink_paused(true, 0, 6_000, 1000, 8_000));
    }

    #[test]
    fn long_idle_zero_disables_idle_suspend() {
        // Past the typing window but long-idle disabled → blink.
        assert!(!blink_paused(true, 400, 0, 1000, 60_000));
    }

    #[test]
    fn long_idle_suspends_after_threshold() {
        // α.3: blink stops once we exceed the idle threshold.
        assert!(!blink_paused(true, 400, 6_000, 1000, 6_999));
        assert!(blink_paused(true, 400, 6_000, 1000, 7_000));
        assert!(blink_paused(true, 400, 6_000, 1000, 60_000));
    }

    #[test]
    fn pause_saturating_sub_handles_clock_jitter() {
        // last_input ahead of `now` (clock skew / monotonic jitter):
        // saturating_sub yields 0 < pause → pause active.
        assert!(blink_paused(true, 400, 6_000, 2000, 1500));
    }
}
