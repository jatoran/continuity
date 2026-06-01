//! Text-selection autoscroll while dragging past the focused pane body.
//!
//! The timer and state live on the owning window's UI thread. Core still
//! owns buffer text; this module only scrolls the focused view and extends
//! the current selection through the existing mouse caret-placement path.

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, KillTimer, SetTimer};

use crate::mouse::{Autoscroll, AutoscrollDirection};
use crate::pane_layout::Rect;
use crate::window::Window;
use crate::window_mouse_hover::wall_clock_ms;
use crate::window_timers::{MOUSE_DRAG_AUTOSCROLL_TIMER_ID, MOUSE_DRAG_AUTOSCROLL_TIMER_MS};

const AUTOSCROLL_DEAD_BAND_DIP: f32 = 8.0;
const AUTOSCROLL_RAMP_END_DIP: f32 = 80.0;
const AUTOSCROLL_MIN_LINES_PER_TICK: f32 = 1.0;
const AUTOSCROLL_MAX_LINES_PER_TICK: f32 = 10.0;
const AUTOSCROLL_SCROLL_EPSILON_DIP: f32 = 0.01;

#[derive(Debug, Clone, Copy)]
struct AutoscrollRequest {
    direction: AutoscrollDirection,
    distance_dip: i32,
    lines_per_tick: f32,
}

#[derive(Debug, Clone, Copy)]
enum AutoscrollStopReason {
    EdgeExit,
    CaptureLost,
    BufferEnd,
    ButtonUp,
}

impl AutoscrollStopReason {
    fn as_trace_str(self) -> &'static str {
        match self {
            Self::EdgeExit => "edge_exit",
            Self::CaptureLost => "capture_lost",
            Self::BufferEnd => "buffer_end",
            Self::ButtonUp => "button_up",
        }
    }
}

impl Window {
    pub(crate) fn begin_selection_drag(&mut self, x: i32, y: i32) {
        let body = self.focused_body_rect();
        if !body.contains(x as f32, y as f32) {
            self.mouse_state.selection_drag_pane = None;
            self.stop_mouse_drag_autoscroll(AutoscrollStopReason::EdgeExit);
            return;
        }
        self.mouse_state.selection_drag_pane = Some(self.tree.focused);
        self.stop_mouse_drag_autoscroll(AutoscrollStopReason::EdgeExit);
        if self.hwnd.0 as isize != 0 {
            unsafe {
                let _ = SetCapture(self.hwnd);
            }
        }
    }

    pub(crate) fn extend_drag_selection_at_pixel(&mut self, x: i32, y: i32) -> bool {
        let before = crate::paint_trace::is_trace_enabled()
            .then(|| {
                self.editor
                    .snapshot(self.buffer_id)
                    .map(|snapshot| snapshot.selections().to_vec())
            })
            .flatten();
        let placed = self.place_caret_at_pixel(x, y, true);
        if crate::paint_trace::is_trace_enabled() {
            let after = self
                .editor
                .snapshot(self.buffer_id)
                .map(|snapshot| snapshot.selections().to_vec())
                .unwrap_or_default();
            let selection_changed = before
                .as_ref()
                .map(|prior| prior.as_slice() != after.as_slice())
                .unwrap_or(placed);
            crate::paint_trace::log_event(
                "mouse_drag_selection",
                &format!(
                    "x={x} y={y} selection_changed={selection_changed} placed={placed} selections={}",
                    after.len()
                ),
            );
        }
        placed
    }

    pub(crate) fn update_mouse_drag_autoscroll_from_cursor(&mut self, x: i32, y: i32) {
        if !self.is_mouse_drag_autoscroll_eligible() {
            self.stop_mouse_drag_autoscroll(AutoscrollStopReason::EdgeExit);
            return;
        }
        let body = self.focused_body_rect();
        let Some(request) = compute_autoscroll_request(body, y as f32) else {
            self.stop_mouse_drag_autoscroll(AutoscrollStopReason::EdgeExit);
            return;
        };
        self.apply_mouse_drag_autoscroll_state(x, y, request);
    }

    pub(crate) fn finish_selection_drag_for_button_up(&mut self) -> bool {
        self.finish_selection_drag(AutoscrollStopReason::ButtonUp)
    }

    pub(crate) fn on_capture_changed(&mut self) {
        let had_selection_drag = self.finish_selection_drag(AutoscrollStopReason::CaptureLost);
        if had_selection_drag {
            self.mouse_state.dragging = false;
        }
        // Losing capture (Alt+Tab, system pop-up, etc.) mid-drag must
        // tear down the tab drag — otherwise the source tab stays
        // faded and the next mouse click runs into a stale drop
        // affordance. Treated as a cancel: no commit.
        self.mouse_state.minimap_dragging = false;
        let _ = self.cancel_tab_drag();
    }

    pub(crate) fn on_mouse_drag_autoscroll_timer(&mut self, hwnd: HWND) {
        if self.tick_mouse_drag_autoscroll(hwnd) {
            self.invalidate(hwnd);
        }
    }

    fn tick_mouse_drag_autoscroll(&mut self, hwnd: HWND) -> bool {
        let Some(state) = self.mouse_state.autoscroll else {
            return false;
        };
        if !self.is_mouse_drag_autoscroll_eligible() {
            self.stop_mouse_drag_autoscroll(AutoscrollStopReason::CaptureLost);
            return false;
        }
        let (cursor_x, cursor_y) =
            self.current_autoscroll_cursor(hwnd, state.last_cursor_x, state.last_cursor_y);
        let body = self.focused_body_rect();
        let Some(request) = compute_autoscroll_request(body, cursor_y as f32) else {
            self.update_autoscroll_cursor(cursor_x, cursor_y);
            self.stop_mouse_drag_autoscroll(AutoscrollStopReason::EdgeExit);
            return false;
        };
        self.apply_mouse_drag_autoscroll_state(cursor_x, cursor_y, request);

        let lines_to_advance = if self.motion_policy().is_reduced_motion() {
            compute_whole_line_autoscroll_lines(request.lines_per_tick)
        } else {
            request.lines_per_tick
        };
        let before_scroll_y_dip = self.view.scroll_y_dip;
        let _ = self.view_scroll_lines_impl(lines_to_advance);
        let after_scroll_y_dip = self.view.scroll_y_dip;
        let scroll_delta_dip = after_scroll_y_dip - before_scroll_y_dip;
        let lines_advanced = (scroll_delta_dip / self.effective_line_height()).round() as i32;

        let (selection_x, selection_y) = clamp_cursor_to_body(body, cursor_x, cursor_y);
        let selection_changed = self.extend_drag_selection_at_pixel(selection_x, selection_y);
        log_autoscroll_event(
            "tick",
            request.direction,
            request.distance_dip,
            lines_advanced,
            AutoscrollStopReason::EdgeExit,
            self.elapsed_mouse_drag_autoscroll_ms(),
        );

        if !has_scroll_advanced(before_scroll_y_dip, after_scroll_y_dip, request.direction) {
            self.stop_mouse_drag_autoscroll(AutoscrollStopReason::BufferEnd);
            return selection_changed;
        }
        true
    }

    fn finish_selection_drag(&mut self, reason: AutoscrollStopReason) -> bool {
        let had_selection_drag = self.mouse_state.selection_drag_pane.take().is_some();
        self.stop_mouse_drag_autoscroll(reason);
        if had_selection_drag && self.hwnd.0 as isize != 0 {
            unsafe {
                let _ = ReleaseCapture();
            }
        }
        had_selection_drag
    }

    fn is_mouse_drag_autoscroll_eligible(&self) -> bool {
        self.mouse_state.dragging
            && self.mouse_state.selection_drag_pane == Some(self.tree.focused)
            && self.mouse_state.splitter_drag.is_none()
            && self.mouse_state.tab_drag.is_none()
            && self.mouse_state.scrollbar_drag.is_none()
            && !self.mouse_state.minimap_dragging
            && self.time_machine_drag.is_none()
    }

    fn current_autoscroll_cursor(
        &self,
        hwnd: HWND,
        fallback_x: i32,
        fallback_y: i32,
    ) -> (i32, i32) {
        if hwnd.0 as isize == 0 {
            return (fallback_x, fallback_y);
        }
        let mut point = POINT::default();
        if unsafe { GetCursorPos(&mut point) }.is_err() {
            return (fallback_x, fallback_y);
        }
        if !unsafe { ScreenToClient(hwnd, &mut point).as_bool() } {
            return (fallback_x, fallback_y);
        }
        self.physical_point_to_dip(point.x, point.y)
    }

    fn apply_mouse_drag_autoscroll_state(&mut self, x: i32, y: i32, request: AutoscrollRequest) {
        let now_ms = wall_clock_ms();
        if let Some(state) = self.mouse_state.autoscroll.as_mut() {
            state.last_cursor_x = x;
            state.last_cursor_y = y;
            state.direction = request.direction;
            state.distance_dip = request.distance_dip;
            return;
        }
        self.mouse_state.autoscroll = Some(Autoscroll {
            last_cursor_x: x,
            last_cursor_y: y,
            direction: request.direction,
            distance_dip: request.distance_dip,
            started_ms: now_ms,
        });
        if self.hwnd.0 as isize != 0 {
            unsafe {
                let _ = SetTimer(
                    Some(self.hwnd),
                    MOUSE_DRAG_AUTOSCROLL_TIMER_ID,
                    MOUSE_DRAG_AUTOSCROLL_TIMER_MS,
                    None,
                );
            }
        }
        log_autoscroll_event(
            "start",
            request.direction,
            request.distance_dip,
            0,
            AutoscrollStopReason::EdgeExit,
            0,
        );
    }

    fn update_autoscroll_cursor(&mut self, x: i32, y: i32) {
        if let Some(state) = self.mouse_state.autoscroll.as_mut() {
            state.last_cursor_x = x;
            state.last_cursor_y = y;
        }
    }

    fn stop_mouse_drag_autoscroll(&mut self, reason: AutoscrollStopReason) {
        let Some(state) = self.mouse_state.autoscroll.take() else {
            return;
        };
        if self.hwnd.0 as isize != 0 {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), MOUSE_DRAG_AUTOSCROLL_TIMER_ID);
            }
        }
        let elapsed_ms = elapsed_ms_since(state.started_ms, wall_clock_ms());
        log_autoscroll_event(
            "stop",
            state.direction,
            state.distance_dip,
            0,
            reason,
            elapsed_ms,
        );
    }

    fn elapsed_mouse_drag_autoscroll_ms(&self) -> u32 {
        self.mouse_state
            .autoscroll
            .map(|state| elapsed_ms_since(state.started_ms, wall_clock_ms()))
            .unwrap_or(0)
    }
}

fn compute_autoscroll_request(body: Rect, cursor_y_dip: f32) -> Option<AutoscrollRequest> {
    let signed_distance_dip = if cursor_y_dip < body.y {
        cursor_y_dip - body.y
    } else if cursor_y_dip > body.bottom() {
        cursor_y_dip - body.bottom()
    } else {
        return None;
    };
    let lines_per_tick = compute_autoscroll_lines_per_tick(signed_distance_dip);
    if lines_per_tick.abs() <= f32::EPSILON {
        return None;
    }
    let direction = if lines_per_tick < 0.0 {
        AutoscrollDirection::Up
    } else {
        AutoscrollDirection::Down
    };
    Some(AutoscrollRequest {
        direction,
        distance_dip: signed_distance_dip.abs().round() as i32,
        lines_per_tick,
    })
}

fn compute_autoscroll_lines_per_tick(distance_dip: f32) -> f32 {
    if !distance_dip.is_finite() {
        return 0.0;
    }
    let magnitude = distance_dip.abs();
    if magnitude <= AUTOSCROLL_DEAD_BAND_DIP {
        return 0.0;
    }
    let direction = distance_dip.signum();
    let ramp_width = AUTOSCROLL_RAMP_END_DIP - AUTOSCROLL_DEAD_BAND_DIP;
    let ramp_progress = ((magnitude.min(AUTOSCROLL_RAMP_END_DIP) - AUTOSCROLL_DEAD_BAND_DIP)
        / ramp_width)
        .clamp(0.0, 1.0);
    direction
        * (AUTOSCROLL_MIN_LINES_PER_TICK
            + ramp_progress * (AUTOSCROLL_MAX_LINES_PER_TICK - AUTOSCROLL_MIN_LINES_PER_TICK))
}

fn compute_whole_line_autoscroll_lines(lines_per_tick: f32) -> f32 {
    if lines_per_tick == 0.0 {
        return 0.0;
    }
    let direction = lines_per_tick.signum();
    let magnitude = lines_per_tick
        .abs()
        .round()
        .clamp(AUTOSCROLL_MIN_LINES_PER_TICK, AUTOSCROLL_MAX_LINES_PER_TICK);
    direction * magnitude
}

fn clamp_cursor_to_body(body: Rect, x: i32, y: i32) -> (i32, i32) {
    let right = (body.right() - 1.0).max(body.x);
    let bottom = (body.bottom() - 1.0).max(body.y);
    (
        (x as f32).clamp(body.x, right).round() as i32,
        (y as f32).clamp(body.y, bottom).round() as i32,
    )
}

fn has_scroll_advanced(before: f32, after: f32, direction: AutoscrollDirection) -> bool {
    match direction {
        AutoscrollDirection::Up => before - after > AUTOSCROLL_SCROLL_EPSILON_DIP,
        AutoscrollDirection::Down => after - before > AUTOSCROLL_SCROLL_EPSILON_DIP,
    }
}

fn elapsed_ms_since(started_ms: u64, now_ms: u64) -> u32 {
    u32::try_from(now_ms.saturating_sub(started_ms)).unwrap_or(u32::MAX)
}

fn log_autoscroll_event(
    state: &'static str,
    direction: AutoscrollDirection,
    distance_dip: i32,
    lines_advanced: i32,
    reason: AutoscrollStopReason,
    elapsed_ms_since_start: u32,
) {
    if !crate::paint_trace::is_trace_enabled() {
        return;
    }
    crate::paint_trace::log_event(
        "mouse_drag_autoscroll",
        &format!(
            "state={state} direction={} distance_dip={distance_dip} lines_advanced={lines_advanced} reason={} elapsed_ms_since_start={elapsed_ms_since_start}",
            direction.as_trace_str(),
            reason.as_trace_str(),
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_approx_eq(left: f32, right: f32) {
        assert!((left - right).abs() < 0.001, "left={left} right={right}");
    }

    #[test]
    fn speed_curve_has_dead_band() {
        assert_eq!(compute_autoscroll_lines_per_tick(0.0), 0.0);
        assert_eq!(compute_autoscroll_lines_per_tick(8.0), 0.0);
        assert_eq!(compute_autoscroll_lines_per_tick(-8.0), 0.0);
    }

    #[test]
    fn speed_curve_ramps_linearly() {
        assert_approx_eq(compute_autoscroll_lines_per_tick(44.0), 5.5);
        assert_approx_eq(compute_autoscroll_lines_per_tick(-44.0), -5.5);
    }

    #[test]
    fn speed_curve_clamps_at_maximum() {
        assert_approx_eq(compute_autoscroll_lines_per_tick(80.0), 10.0);
        assert_approx_eq(compute_autoscroll_lines_per_tick(200.0), 10.0);
        assert_approx_eq(compute_autoscroll_lines_per_tick(-200.0), -10.0);
    }

    #[test]
    fn stop_predicate_ignores_inside_and_exact_edges() {
        let body = Rect::new(10.0, 20.0, 100.0, 200.0);
        assert!(compute_autoscroll_request(body, 100.0).is_none());
        assert!(compute_autoscroll_request(body, 20.0).is_none());
        assert!(compute_autoscroll_request(body, 220.0).is_none());
        assert!(compute_autoscroll_request(body, 12.0).is_none());
        assert!(compute_autoscroll_request(body, 228.0).is_none());
    }

    #[test]
    fn stop_predicate_starts_past_dead_band() {
        let body = Rect::new(10.0, 20.0, 100.0, 200.0);
        let below = compute_autoscroll_request(body, 229.0)
            .expect("one dip past dead band below bottom should autoscroll");
        assert_eq!(below.direction, AutoscrollDirection::Down);
        assert!(below.lines_per_tick > 0.0);
        let above = compute_autoscroll_request(body, 11.0)
            .expect("one dip past dead band above top should autoscroll");
        assert_eq!(above.direction, AutoscrollDirection::Up);
        assert!(above.lines_per_tick < 0.0);
    }
}
