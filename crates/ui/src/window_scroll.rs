//! Wheel-scroll inertia state and helpers.
//!
//! Thread ownership: [`crate::Window`]'s UI thread owns and mutates the
//! single [`ScrollInertia`] value. The core buffer thread never sees this
//! state; paint receives only the resulting fractional `scroll_y_dip`.

use continuity_layout::ViewState;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::SystemInformation::GetTickCount64;

use crate::pane_layout::{metrics, pane_at_point, Rect};
use crate::pane_state::PerPaneState;
use crate::pane_tree::{PaneId, PaneTree};
use crate::window::{Window, WHEEL_LINES_PER_NOTCH};
use crate::window_font_picker::compute_overscroll_bottom_dip;

/// Exponential decay time constant for wheel inertia. At 60 ms, velocity
/// reaches 10% in about 138 ms, so wheel flicks still feel continuous but
/// stop close to when the user's fingers stop moving.
pub(crate) const INERTIA_TIME_CONSTANT_MS: f32 = 60.0;
/// Clamp the sub-DIP tail: 50 DIP/s is roughly 0.8 DIP per 60 Hz frame,
/// below the point where continued crawling reads as intentional motion.
const INERTIA_STOP_THRESHOLD_DIP_PER_S: f32 = 50.0;
const WHEEL_IMPULSE_WINDOW_MS: f32 = 80.0;
const MAX_TICK_DT_MS: u64 = 50;
/// Minimum "ahead-of-scroll" distance (DIPs) the most-recently-painted
/// frame's realized window must hold to skip the sliding-window prewarm.
/// When the gap drops below this threshold during active inertia, the
/// post-paint hook submits a worker request one viewport-page ahead so
/// the next scroll-tick paint hits a covering frame instead of falling
/// through to the placeholder strip.
pub(crate) const SLIDING_PREWARM_LEAD_RATIO: f32 = 0.5;

/// UI-thread scroll momentum accumulator.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ScrollInertia {
    velocity_dip_per_s: f32,
    last_tick_ms: u64,
    target_pane: Option<PaneId>,
    hover_routed: bool,
    active: bool,
}

/// Result of one inertia tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ScrollInertiaTick {
    pub(crate) moved: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ScrollInertiaStep {
    target_pane: PaneId,
    delta_dip: f32,
}

impl ScrollInertia {
    /// `true` while decay should keep ticking.
    #[must_use]
    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    /// Current velocity. Returns zero after the state has stopped.
    #[must_use]
    pub(crate) fn velocity_dip_per_s(&self) -> f32 {
        if self.active {
            self.velocity_dip_per_s
        } else {
            0.0
        }
    }

    /// Current target pane while active.
    #[must_use]
    pub(crate) fn target_pane(&self) -> Option<PaneId> {
        self.active.then_some(self.target_pane).flatten()
    }

    /// `true` when the active impulse landed on a non-focused pane.
    #[must_use]
    pub(crate) fn hover_routed(&self) -> bool {
        self.active && self.hover_routed
    }

    /// Stop the inertial scroll immediately.
    pub(crate) fn cancel(&mut self) {
        *self = Self::default();
    }

    /// Add one wheel impulse to the current velocity. Concurrent wheel
    /// notches stack by design when they target the same pane; a new
    /// target starts a fresh impulse so one flick never scrolls two panes.
    pub(crate) fn add_wheel_delta(
        &mut self,
        target_pane: PaneId,
        focused_pane: PaneId,
        delta_dip: f32,
        now_ms: u64,
    ) {
        if !self.active || self.target_pane != Some(target_pane) {
            self.velocity_dip_per_s = 0.0;
            self.last_tick_ms = now_ms;
            self.target_pane = Some(target_pane);
            self.hover_routed = target_pane != focused_pane;
            self.active = true;
        }
        self.velocity_dip_per_s += delta_dip * (1000.0 / WHEEL_IMPULSE_WINDOW_MS);
    }

    fn next_step(&mut self, now_ms: u64) -> Option<ScrollInertiaStep> {
        if !self.active {
            return None;
        }
        let target_pane = match self.target_pane {
            Some(target) => target,
            None => {
                self.cancel();
                return None;
            }
        };
        let elapsed_ms = now_ms
            .saturating_sub(self.last_tick_ms)
            .clamp(1, MAX_TICK_DT_MS);
        self.last_tick_ms = now_ms;
        let dt_s = elapsed_ms as f32 / 1000.0;
        let tau_s = INERTIA_TIME_CONSTANT_MS / 1000.0;
        self.velocity_dip_per_s *= (-dt_s / tau_s).exp();
        if self.velocity_dip_per_s.abs() < INERTIA_STOP_THRESHOLD_DIP_PER_S {
            self.cancel();
            return None;
        }
        Some(ScrollInertiaStep {
            target_pane,
            delta_dip: self.velocity_dip_per_s * dt_s,
        })
    }

    /// Advance the inertial scroll by elapsed time.
    #[cfg(test)]
    fn tick(
        &mut self,
        view: &mut ViewState,
        content_height_dip: f32,
        now_ms: u64,
    ) -> ScrollInertiaTick {
        let Some(step) = self.next_step(now_ms) else {
            return ScrollInertiaTick::default();
        };
        let before = view.scroll_y_dip;
        view.scroll_instant(step.delta_dip, content_height_dip);
        let moved = (view.scroll_y_dip - before).abs() > f32::EPSILON;
        if !moved {
            self.cancel();
        }
        ScrollInertiaTick { moved }
    }
}

/// Normalize a user-configurable wheel-speed multiplier.
#[must_use]
pub(crate) fn compute_wheel_scroll_speed(speed: f32) -> f32 {
    if speed.is_finite() && speed > 0.0 {
        speed
    } else {
        1.0
    }
}

/// Convert wheel notches into the configured line-step delta. `line_height`
/// is the current (zoom-scaled) row stride so one notch always moves the
/// same number of *lines* regardless of zoom.
#[must_use]
pub(crate) fn wheel_delta_dip(notches: f32, speed: f32, line_height: f32) -> f32 {
    -notches * WHEEL_LINES_PER_NOTCH * compute_wheel_scroll_speed(speed) * line_height
}

/// Resolve a wheel point to the pane body that should scroll.
#[must_use]
pub(crate) fn wheel_target_pane_at_point(
    tree: &PaneTree,
    root_rect: Rect,
    x: f32,
    y: f32,
) -> Option<PaneId> {
    let (pane, outer) = pane_at_point(tree, root_rect, x, y)?;
    let strip = metrics::TAB_STRIP_HEIGHT_DIP.min(outer.h);
    let body = Rect::new(
        outer.x,
        outer.y + strip,
        outer.w,
        (outer.h - strip).max(0.0),
    );
    body.contains(x, y).then_some(pane)
}

/// Signed lead distance (DIPs) the cached frame's realized window
/// holds ahead of the live scroll position in the direction of
/// `velocity`. Positive means the realized window extends ahead of
/// the visible viewport; negative means it has fallen behind.
///
/// Pure function so the sliding-window prewarm predicate is testable
/// without a `Window`.
#[must_use]
pub(crate) fn realized_lead_ahead_dip(
    realized_start_dip: f32,
    realized_end_dip: f32,
    scroll_y_dip: f32,
    viewport_h_dip: f32,
    velocity_dip_per_s: f32,
) -> f32 {
    if velocity_dip_per_s > 0.0 {
        // Scrolling down — leading edge is the bottom of realized.
        realized_end_dip - (scroll_y_dip + viewport_h_dip)
    } else if velocity_dip_per_s < 0.0 {
        // Scrolling up — leading edge is the top of realized.
        scroll_y_dip - realized_start_dip
    } else {
        f32::INFINITY
    }
}

/// Sliding-window prewarm decision: when active inertia carries the
/// scroll position close enough to the trailing edge of the cached
/// frame's realized window, submit a worker request one viewport-page
/// ahead of the live scroll so the next scroll-tick paint hits a
/// covering frame.
#[must_use]
pub(crate) fn should_submit_sliding_prewarm(
    realized_start_dip: f32,
    realized_end_dip: f32,
    scroll_y_dip: f32,
    viewport_h_dip: f32,
    velocity_dip_per_s: f32,
) -> bool {
    if velocity_dip_per_s.abs() < f32::EPSILON || viewport_h_dip <= 0.0 {
        return false;
    }
    let lead = realized_lead_ahead_dip(
        realized_start_dip,
        realized_end_dip,
        scroll_y_dip,
        viewport_h_dip,
        velocity_dip_per_s,
    );
    lead < SLIDING_PREWARM_LEAD_RATIO * viewport_h_dip
}

/// Reduced-motion wheel handling keeps whole-line jumps and no decay.
/// `line_height` is the current (zoom-scaled) row stride.
#[must_use]
pub(crate) fn reduced_motion_wheel_delta_dip(notches: f32, speed: f32, line_height: f32) -> f32 {
    (-notches * WHEEL_LINES_PER_NOTCH * compute_wheel_scroll_speed(speed)).round() * line_height
}

impl Window {
    /// Cancel active wheel inertia without touching any discrete
    /// `ViewState` animation.
    pub(crate) fn cancel_scroll_inertia(&mut self) {
        self.scroll_inertia.cancel();
    }

    /// Current scroll inertia velocity for render tracing.
    #[must_use]
    pub(crate) fn scroll_velocity_dip_per_s(&self) -> f32 {
        self.scroll_inertia.velocity_dip_per_s()
    }

    /// Fields appended to `event:scroll_path`.
    #[must_use]
    pub(crate) fn scroll_trace_state(&self) -> (u128, u128, bool) {
        let target = self
            .scroll_inertia
            .target_pane()
            .map_or(0, |pane| u128::from(pane.0));
        (
            target,
            u128::from(self.tree.focused.0),
            self.scroll_inertia.hover_routed(),
        )
    }

    /// Resolve a client-DIP wheel point to the hovered pane body.
    #[must_use]
    pub(crate) fn wheel_scroll_target_at(&self, x: i32, y: i32) -> Option<PaneId> {
        wheel_target_pane_at_point(&self.tree, self.pane_root_rect(), x as f32, y as f32)
    }

    /// Apply a wheel delta through the P12 inertia model, or through
    /// the reduced-motion instant path. Section 10 removed the
    /// per-wheel landing-viewport prewarm because it produced ~98 %
    /// stamp-mismatch waste (one worker hit per 68 submissions in
    /// `perf-snapshots/trace_20260521-225551.report.md`); the
    /// post-paint sliding-window prewarm
    /// ([`Self::maybe_submit_sliding_scroll_prewarm`]) replaces it.
    pub(crate) fn apply_wheel_scroll(
        &mut self,
        hwnd: HWND,
        target_pane: PaneId,
        notches: f32,
    ) -> bool {
        let Some(body_rect) = self.pane_body_rect(target_pane) else {
            return false;
        };
        let content_height_dip = self.estimated_content_height_for_pane(target_pane);
        let wheel_speed = self.view_options.mouse_wheel_scroll_speed;
        let line_height = self.effective_line_height();
        let overscroll_bottom = compute_overscroll_bottom_dip(
            self.view_options.scroll_past_end,
            body_rect.h,
            line_height,
        );
        if self.motion_policy().is_reduced_motion() || !self.view_options.smooth_scroll {
            self.cancel_scroll_inertia();
            let moved = self
                .with_scroll_target_view_mut(target_pane, |view| {
                    view.viewport_width_dip = body_rect.w;
                    view.viewport_height_dip = body_rect.h;
                    view.overscroll_bottom_dip = overscroll_bottom;
                    view.line_height_dip = line_height;
                    let before = view.scroll_y_dip;
                    view.scroll_instant(
                        reduced_motion_wheel_delta_dip(notches, wheel_speed, line_height),
                        content_height_dip,
                    );
                    (view.scroll_y_dip - before).abs() > f32::EPSILON
                })
                .unwrap_or(false);
            if !self.view.animating() {
                self.stop_scroll_anim(hwnd);
            }
            return moved;
        }
        let now_ms = unsafe { GetTickCount64() };
        let Some(()) = self.with_scroll_target_view_mut(target_pane, |view| {
            view.viewport_width_dip = body_rect.w;
            view.viewport_height_dip = body_rect.h;
            view.overscroll_bottom_dip = overscroll_bottom;
            view.line_height_dip = line_height;
            view.cancel_scroll_animation();
        }) else {
            return false;
        };
        self.scroll_inertia.add_wheel_delta(
            target_pane,
            self.tree.focused,
            wheel_delta_dip(notches, wheel_speed, line_height),
            now_ms,
        );
        self.start_scroll_anim(hwnd);
        // The inertia timer owns painting for smooth wheel scroll. Asking
        // WM_MOUSEWHEEL to invalidate too can interleave raw wheel paints with
        // timer paints and push renderer EndDraw into stalls.
        false
    }

    /// Post-paint sliding-window prewarm. When wheel inertia is
    /// active and the just-painted frame's realized window holds less
    /// than [`SLIDING_PREWARM_LEAD_RATIO`] × viewport-height ahead of
    /// the live scroll position (in the direction of velocity),
    /// submit a worker request one viewport-page ahead. Submission
    /// goes through the existing P0.8.2 early-dispatch path with
    /// `submit_reason="scroll_prewarm"` so the perf trace can isolate
    /// it. Cadence: one check per paint, gated by the predicate; the
    /// worker channel's stamp dedupe filters duplicates.
    pub(crate) fn maybe_submit_sliding_scroll_prewarm(
        &mut self,
        realized_start_row: u32,
        realized_end_row: u32,
    ) {
        if !self.is_scroll_inertia_active() {
            return;
        }
        if self.scroll_inertia.target_pane() != Some(self.tree.focused) {
            return;
        }
        let velocity = self.scroll_inertia.velocity_dip_per_s();
        let viewport_h = self.view.viewport_height_dip;
        let scroll_y = self.view.scroll_y_dip;
        let line_height = self.effective_line_height();
        let realized_start_dip = realized_start_row as f32 * line_height;
        let realized_end_dip = realized_end_row as f32 * line_height;
        if !should_submit_sliding_prewarm(
            realized_start_dip,
            realized_end_dip,
            scroll_y,
            viewport_h,
            velocity,
        ) {
            return;
        }
        let content_height_dip = self.estimated_content_height();
        let max_scroll = (content_height_dip - viewport_h).max(0.0);
        let target_scroll_y = (scroll_y + velocity.signum() * viewport_h).clamp(0.0, max_scroll);
        let viewport_rows = crate::window_paint::visible_display_row_range(
            target_scroll_y,
            viewport_h,
            line_height,
        );
        let _ = self.try_dispatch_projection_worker_early_with_viewport(
            "scroll_prewarm",
            "scroll_prewarm",
            Some(viewport_rows),
        );
    }

    /// Tick inertial wheel scrolling on the existing scroll animation
    /// timer.
    pub(crate) fn tick_scroll_inertia(&mut self) -> ScrollInertiaTick {
        let now_ms = unsafe { GetTickCount64() };
        let Some(step) = self.scroll_inertia.next_step(now_ms) else {
            return ScrollInertiaTick::default();
        };
        let Some(body_rect) = self.pane_body_rect(step.target_pane) else {
            self.scroll_inertia.cancel();
            return ScrollInertiaTick::default();
        };
        let content_height_dip = self.estimated_content_height_for_pane(step.target_pane);
        let line_height = self.effective_line_height();
        let overscroll_bottom = compute_overscroll_bottom_dip(
            self.view_options.scroll_past_end,
            body_rect.h,
            line_height,
        );
        let moved = self
            .with_scroll_target_view_mut(step.target_pane, |view| {
                view.viewport_width_dip = body_rect.w;
                view.viewport_height_dip = body_rect.h;
                view.overscroll_bottom_dip = overscroll_bottom;
                view.line_height_dip = line_height;
                let before = view.scroll_y_dip;
                view.scroll_instant(step.delta_dip, content_height_dip);
                (view.scroll_y_dip - before).abs() > f32::EPSILON
            })
            .unwrap_or(false);
        if !moved {
            self.scroll_inertia.cancel();
        }
        ScrollInertiaTick { moved }
    }

    /// Whether wheel inertia still needs the scroll timer.
    #[must_use]
    pub(crate) fn is_scroll_inertia_active(&self) -> bool {
        self.scroll_inertia.is_active()
    }

    fn estimated_content_height_for_pane(&self, pane: PaneId) -> f32 {
        if pane == self.tree.focused {
            return self.estimated_content_height();
        }
        let Some(buffer_id) = self.buffer_id_for_pane(pane) else {
            return 0.0;
        };
        let line_height = self.effective_line_height();
        if let Some(rows) = self
            .spectator_frame_cache
            .borrow()
            .display_line_count(pane, buffer_id)
        {
            return rows.max(1) as f32 * line_height;
        }
        let Some(snap) = self.editor.snapshot(buffer_id) else {
            return 0.0;
        };
        let lines = snap.rope_snapshot().rope().len_lines().max(1) as f32;
        lines * line_height
    }

    fn buffer_id_for_pane(&self, pane: PaneId) -> Option<continuity_buffer::BufferId> {
        if pane == self.tree.focused {
            return Some(self.buffer_id);
        }
        let group = self.tree.groups.get(&pane)?;
        let tab = self.tree.tabs.get(&group.active)?;
        Some(tab.buffer_id)
    }

    fn with_scroll_target_view_mut<R>(
        &mut self,
        pane: PaneId,
        f: impl FnOnce(&mut ViewState) -> R,
    ) -> Option<R> {
        if pane == self.tree.focused {
            return Some(f(&mut self.view));
        }
        let buffer_id = self.buffer_id_for_pane(pane)?;
        let state = self
            .panes
            .entry(pane)
            .or_insert_with(|| PerPaneState::new(buffer_id, Self::default_language()));
        if state.buffer_id != buffer_id {
            *state = PerPaneState::new(buffer_id, Self::default_language());
        }
        Some(f(&mut state.view))
    }
}

#[cfg(test)]
mod tests;
