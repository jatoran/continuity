//! δ.2 — idle-stale status-bar segment formatter.
//!
//! Builds the "idle Xm" / "idle Xh" / "idle XhYm" label rendered by
//! [`continuity_config::StatusBarSegment::IdleStale`]. Pure function
//! over the millis-since-last-input value so it's testable without an
//! HWND; the caller (`window_status_bar::build_segment`) supplies the
//! elapsed time computed against [`Window::last_input_tick`].
//!
//! Suppression: returns `None` while the editor has been idle for
//! less than [`IDLE_STALE_THRESHOLD_MS`] (5 minutes). The segment
//! disappears entirely below threshold rather than rendering "idle
//! 0m", because the goal is to confirm a session is still alive after
//! a long pause without ever shouting at the user during normal use.

/// Threshold (ms) below which the idle-stale segment is suppressed.
pub(crate) const IDLE_STALE_THRESHOLD_MS: u64 = 5 * 60 * 1_000;

/// Format the "idle Xm ago" / "idle Xh ago" label. Returns `None`
/// below [`IDLE_STALE_THRESHOLD_MS`].
pub(crate) fn format_idle_stale(idle_ms: u64) -> Option<String> {
    if idle_ms < IDLE_STALE_THRESHOLD_MS {
        return None;
    }
    let minutes = idle_ms / 60_000;
    if minutes < 60 {
        return Some(format!("idle {minutes}m"));
    }
    let hours = minutes / 60;
    let extra = minutes % 60;
    if extra == 0 {
        Some(format!("idle {hours}h"))
    } else {
        Some(format!("idle {hours}h{extra}m"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suppressed_below_threshold() {
        assert!(format_idle_stale(0).is_none());
        assert!(format_idle_stale(IDLE_STALE_THRESHOLD_MS - 1).is_none());
    }

    #[test]
    fn minutes_format() {
        let s = format_idle_stale(IDLE_STALE_THRESHOLD_MS).unwrap();
        assert_eq!(s, "idle 5m");
        let s = format_idle_stale(7 * 60_000).unwrap();
        assert_eq!(s, "idle 7m");
    }

    #[test]
    fn hour_compact() {
        let s = format_idle_stale(60 * 60_000).unwrap();
        assert_eq!(s, "idle 1h");
        let s = format_idle_stale(90 * 60_000).unwrap();
        assert_eq!(s, "idle 1h30m");
    }
}
