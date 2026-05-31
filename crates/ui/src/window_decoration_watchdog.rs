//! UI-thread bridge for decoration worker watchdog events.
//!
//! The decoration pool owns restart decisions. The window only polls the
//! pool's bounded restart-event channel, refreshes the status-bar notice,
//! and asks for a repaint so the existing status-bar chip lane can show it.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::SetTimer;

use crate::window::{DECORATION_WATCHDOG_TIMER_ID, DECORATION_WATCHDOG_TIMER_MS};
use crate::Window;

impl Window {
    /// Start the low-frequency timer that drains decoration watchdog events.
    pub(crate) fn start_decoration_watchdog_poll(&mut self, hwnd: HWND) {
        if self.decoration_watchdog_poll_active || self.decorate_pool.is_none() {
            return;
        }
        unsafe {
            let _ = SetTimer(
                Some(hwnd),
                DECORATION_WATCHDOG_TIMER_ID,
                DECORATION_WATCHDOG_TIMER_MS,
                None,
            );
        }
        self.decoration_watchdog_poll_active = true;
    }

    /// Poll watchdog restart events and status-notice expiry/fade.
    ///
    /// Also drains any decoration results that arrived from the worker
    /// pool between paints. Without this, results sit unused in the
    /// pool's channel until some other UI event triggers a `WM_PAINT`
    /// (mouse move, tab switch, etc.) — which manifested as table
    /// chrome / markdown styling not appearing after open until the
    /// user clicked something. With the 250 ms watchdog cadence the
    /// repaint follows the worker result by at most one tick.
    ///
    /// The same safety net now covers the projection worker. The
    /// off-thread jump's only repaint pump is its bounded poll budget
    /// ([`crate::window::Window::arm_offthread_jump`]); when a focus /
    /// jump build outlasts that budget the realized frame lands in the
    /// result queue with no paint scheduled to accept it — the
    /// "focus-into-buffer is blank until I type" regression. (The motion
    /// timer and doc-end re-invalidation loops that used to pump that
    /// paint incidentally were removed.) Scheduling the realize paint here
    /// makes the budget a latency optimization rather than a correctness
    /// crutch, with the same at-most-one-tick latency.
    pub(crate) fn on_decoration_watchdog_tick(&mut self, hwnd: HWND) {
        let had_notice = !self.status_notices.is_empty();
        let changed = self.drain_decoration_watchdog_events(self.now_ms());
        let decorations_updated = self.drain_decoration_results();
        let projection_ready = self
            .projection_worker
            .as_ref()
            .is_some_and(|worker| worker.has_unconsumed_result());
        if changed || had_notice || decorations_updated || projection_ready {
            // Funnel through `invalidate_with_reason` so the trace
            // names the paint trigger; `InvalidateRect` directly
            // would skip the reason stamp. Decoration delivery takes the
            // reason when both fire — its label is the established one and
            // the same paint drains the projection result too.
            let reason = if decorations_updated {
                "decoration_delivered"
            } else if projection_ready {
                "projection_delivered"
            } else {
                "decoration_watchdog"
            };
            self.invalidate_with_reason(hwnd, reason);
        }
    }

    /// Drain restart events. Returns `true` when visible status state changed.
    pub(crate) fn drain_decoration_watchdog_events(&mut self, now_ms: u64) -> bool {
        let restarts = self
            .decorate_pool
            .as_ref()
            .map(|pool| pool.worker_restarts().try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        let mut changed = false;
        if !restarts.is_empty() {
            for _restart in restarts {
                crate::window_status_notice::push_decoration_restart_notice(
                    &mut self.status_notices,
                    now_ms,
                );
            }
            self.last_submitted_decoration_revision = None;
            self.last_submitted_decoration_revision_per_buffer
                .borrow_mut()
                .clear();
            self.maybe_submit_decoration();
            self.submit_decorations_for_visible_panes();
            changed = true;
        }
        changed |=
            crate::window_status_notice::retain_live_notices(&mut self.status_notices, now_ms);
        changed
    }
}
