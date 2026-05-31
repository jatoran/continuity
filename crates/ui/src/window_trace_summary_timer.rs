//! UI-thread arm for the running-summary trace flush timer.
//!
//! Arms [`crate::window_timers::TRACE_SUMMARY_TIMER_ID`] when
//! `CONTINUITY_UI_TRACE` is set, at the cadence configured by
//! `CONTINUITY_TRACE_SUMMARY_MS` (default 2000 ms). The tick handler
//! lives in [`crate::paint_trace_summary::tick`] and emits one
//! `event:running_summary` line per registered label.
//!
//! Disabling the flush via `CONTINUITY_TRACE_SUMMARY_MS=0` is honoured
//! here by not calling `SetTimer`; the tick handler is harmless when
//! invoked manually.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::SetTimer;

use crate::paint_trace_summary;
use crate::window_timers::TRACE_SUMMARY_TIMER_ID;
use crate::Window;

impl Window {
    /// Arm the running-summary flush timer when tracing is on and the
    /// flush cadence is non-zero. No-op otherwise. Safe to call
    /// repeatedly; Win32 `SetTimer` is idempotent for the same id +
    /// hwnd pair.
    pub(crate) fn start_trace_summary_timer(&self, hwnd: HWND) {
        if !crate::paint_trace::is_trace_enabled() {
            return;
        }
        let Some(interval) = paint_trace_summary::flush_interval() else {
            return;
        };
        let ms = u32::try_from(interval.as_millis()).unwrap_or(u32::MAX);
        unsafe {
            let _ = SetTimer(Some(hwnd), TRACE_SUMMARY_TIMER_ID, ms, None);
        }
    }
}
