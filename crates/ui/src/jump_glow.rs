//! Phase B6 caret-jump acknowledgement glow.
//!
//! When the primary caret moves more than a configurable threshold of
//! display rows (or any cross-buffer / cross-pane jump), the destination
//! row briefly glows for the motion-contract acknowledgement window
//! then fades. Triggers: `goto_line`,
//! `goto_heading`, `find_next` / `find_prev`, click outside viewport,
//! restore from the I1 timeline scrubber. Routine arrow / page / scroll
//! motion is excluded.
//!
//! This module owns the data model and the pure decision functions —
//! the paint side is wired into `window_paint.rs` via the active glow
//! state on `Window`. Theme key: `editor.caret_jump_glow`.

/// Default vertical-distance threshold (in display rows) above which a
/// motion-driven caret movement triggers the glow. Cross-buffer and
/// cross-pane jumps always trigger regardless of distance.
pub(crate) const JUMP_GLOW_THRESHOLD_ROWS: u32 = 3;

use windows::Win32::System::SystemInformation::GetTickCount64;

use crate::Window;

impl Window {
    /// Record the primary caret's pre-motion line. Returns the captured
    /// line (or `None` if no caret / snapshot). Callers should pair
    /// this with `maybe_trigger_jump_glow` *after* the motion lands.
    pub(crate) fn capture_caret_line_for_jump(&self) -> Option<u32> {
        let snap = self.current_snapshot()?;
        snap.selections.first().map(|s| s.head.line)
    }

    /// Inspect the live caret line and, if the move from `from_line`
    /// exceeded [`JUMP_GLOW_THRESHOLD_ROWS`] (or `from_line` is `None`
    /// — used for cross-buffer / cross-pane jumps), arm the glow.
    pub(crate) fn maybe_trigger_jump_glow(&mut self, from_line: Option<u32>) {
        if self.motion_policy().is_reduced_motion() {
            self.jump_glow = None;
            return;
        }
        let Some(snap) = self.current_snapshot() else {
            return;
        };
        let Some(sel) = snap.selections.first() else {
            return;
        };
        let to_line = sel.head.line;
        if should_glow(from_line, to_line, JUMP_GLOW_THRESHOLD_ROWS) {
            self.jump_glow = Some(JumpGlow {
                line: to_line,
                started_ms: unsafe { GetTickCount64() },
            });
            self.start_motion_timer();
        }
    }

    /// Drop the glow if its fade window has elapsed. Called from the
    /// blink tick so eviction piggybacks on existing wakeups.
    pub(crate) fn evict_expired_jump_glow(&mut self) {
        if let Some(g) = self.jump_glow {
            let now = unsafe { GetTickCount64() };
            if fade_alpha(g, now, u64::from(crate::motion::ACK_MOTION_MS)).is_none() {
                self.jump_glow = None;
            }
        }
    }
}

/// One-shot glow decoration applied to a single source line.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct JumpGlow {
    /// 0-based source line index of the destination row.
    pub line: u32,
    /// `GetTickCount64` value captured when the glow was triggered.
    pub started_ms: u64,
}

/// Decide whether a caret move from `from_line` to `to_line` qualifies
/// as a "jump" worthy of the acknowledgement glow.
///
/// Returns `true` when the absolute vertical delta exceeds
/// `threshold_rows`. Cross-buffer / cross-pane jumps should bypass this
/// check and trigger unconditionally — callers in those flows pass a
/// `from_line` of `None` and we return `true`.
pub(crate) fn should_glow(from_line: Option<u32>, to_line: u32, threshold_rows: u32) -> bool {
    let Some(from) = from_line else {
        return true;
    };
    from.abs_diff(to_line) > threshold_rows
}

/// Compute the fade alpha multiplier `[0.0, 1.0]` for an active glow,
/// or return `None` once the fade is complete (caller should evict).
///
/// Cubic ease-out fade from 1.0 at `started_ms` to 0.0 at
/// `started_ms + fade_ms`.
pub(crate) fn fade_alpha(glow: JumpGlow, now_ms: u64, fade_ms: u64) -> Option<f32> {
    let elapsed = now_ms.saturating_sub(glow.started_ms);
    if elapsed >= fade_ms || fade_ms == 0 {
        return None;
    }
    let t = elapsed as f32 / fade_ms as f32;
    Some(1.0 - crate::motion::ease_out_cubic(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glows_on_long_downward_jump() {
        assert!(should_glow(Some(10), 50, 3));
    }

    #[test]
    fn glows_on_long_upward_jump() {
        assert!(should_glow(Some(50), 10, 3));
    }

    #[test]
    fn does_not_glow_for_small_motion() {
        assert!(!should_glow(Some(10), 12, 3));
        assert!(!should_glow(Some(10), 13, 3));
    }

    #[test]
    fn does_not_glow_at_exact_threshold() {
        // > threshold, not >= — exact threshold stays subtle.
        assert!(!should_glow(Some(10), 13, 3));
        assert!(should_glow(Some(10), 14, 3));
    }

    #[test]
    fn cross_buffer_jump_always_glows() {
        assert!(should_glow(None, 0, 3));
        assert!(should_glow(None, 99, 100));
    }

    #[test]
    fn fade_starts_at_one() {
        let g = JumpGlow {
            line: 5,
            started_ms: 1000,
        };
        let alpha = fade_alpha(g, 1000, 300).expect("glow alive");
        assert!((alpha - 1.0).abs() < 1e-6);
    }

    #[test]
    fn fade_midpoint_uses_ease_out() {
        let g = JumpGlow {
            line: 5,
            started_ms: 1000,
        };
        let alpha = fade_alpha(g, 1150, 300).expect("glow alive");
        assert!((alpha - 0.125).abs() < 1e-3);
    }

    #[test]
    fn fade_completes_at_or_after_duration() {
        let g = JumpGlow {
            line: 5,
            started_ms: 1000,
        };
        assert!(fade_alpha(g, 1300, 300).is_none());
        assert!(fade_alpha(g, 5000, 300).is_none());
    }

    #[test]
    fn fade_duration_zero_evicts_immediately() {
        let g = JumpGlow {
            line: 5,
            started_ms: 1000,
        };
        assert!(fade_alpha(g, 1000, 0).is_none());
    }

    #[test]
    fn fade_saturating_handles_clock_jitter() {
        let g = JumpGlow {
            line: 5,
            started_ms: 2000,
        };
        // now_ms < started_ms → saturating_sub yields 0 → fully visible.
        let alpha = fade_alpha(g, 1500, 300).expect("glow alive");
        assert!((alpha - 1.0).abs() < 1e-6);
    }

    #[test]
    fn reduced_motion_policy_schedules_no_jump_glow_frames() {
        assert!(crate::motion::MotionPolicy::new(true)
            .schedule(0, crate::motion::ACK_MOTION_MS)
            .is_none());
    }
}
