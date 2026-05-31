//! Phase B7 caret-motion tween for large edit-driven jumps.
//!
//! When the caret destination is more than `threshold_rows` display
//! rows from the current position **and** the motion is the result of
//! an edit (paste, undo restore, multi-cursor collapse, find-jump),
//! the caret tweens to its new home over the motion-contract duration
//! instead of
//! teleporting. Small motions (arrow keys, neighbor-character edits)
//! stay instant — they don't go through this gate.
//!
//! Settings (Phase B4 plumbing): `editor.caret_tween_enabled`,
//! `editor.caret_tween_threshold_rows`, `editor.caret_tween_duration_ms`.
//! Pairs with B6: the glow marks the destination, the tween carries
//! the eye there.

/// One-shot tween state for the primary caret.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct CaretTween {
    /// Pre-motion display line (the visual position the eye is leaving).
    pub from_line: u32,
    /// Post-motion display line (where the caret has actually landed).
    pub to_line: u32,
    /// `GetTickCount64` value captured when the tween was triggered.
    pub started_ms: u64,
    /// Tween duration in ms; usually matches the
    /// `editor.caret_tween_duration_ms` setting.
    pub duration_ms: u32,
}

/// Decide whether an edit-driven motion from `from_line` to `to_line`
/// warrants a tween. Returns `true` when `enabled` AND the absolute
/// row delta is greater than `threshold_rows`.
pub(crate) fn should_tween(
    enabled: bool,
    from_line: u32,
    to_line: u32,
    threshold_rows: u32,
) -> bool {
    if !enabled {
        return false;
    }
    from_line.abs_diff(to_line) > threshold_rows
}

/// Compute the eased progress `[0.0, 1.0]` of an active tween, or
/// `None` once finished (caller should evict).
///
/// Ease-out cubic — feels smooth at small durations and ends with
/// a near-zero rate so the caret settles without overshoot.
pub(crate) fn tween_progress(tween: CaretTween, now_ms: u64) -> Option<f32> {
    if tween.duration_ms == 0 {
        return None;
    }
    let elapsed = now_ms.saturating_sub(tween.started_ms);
    if elapsed >= u64::from(tween.duration_ms) {
        return None;
    }
    let t = elapsed as f32 / f32::from(tween.duration_ms as u16);
    Some(crate::motion::ease_out_cubic(t))
}

use windows::Win32::System::SystemInformation::GetTickCount64;

use crate::Window;

impl Window {
    /// Arm a caret tween if the just-completed motion qualifies under
    /// the user's settings. Caller passes the pre-motion display line
    /// (typically captured via `capture_caret_line_for_jump`).
    pub(crate) fn maybe_start_caret_tween(&mut self, from_line: u32) {
        if self.motion_policy().is_reduced_motion() {
            self.caret_tween = None;
            return;
        }
        let Some(snap) = self.current_snapshot() else {
            return;
        };
        let Some(sel) = snap.selections.first() else {
            return;
        };
        let to_line = sel.head.line;
        if should_tween(
            self.view_options.caret_tween_enabled,
            from_line,
            to_line,
            self.view_options.caret_tween_threshold_rows,
        ) {
            self.caret_tween = Some(CaretTween {
                from_line,
                to_line,
                started_ms: unsafe { GetTickCount64() },
                duration_ms: self.view_options.caret_tween_duration_ms,
            });
            self.start_motion_timer();
        }
    }

    /// Drop the tween once its duration has elapsed. Piggy-backs on
    /// the caret-blink tick so we avoid a dedicated timer.
    pub(crate) fn evict_expired_caret_tween(&mut self) {
        if let Some(t) = self.caret_tween {
            let now = unsafe { GetTickCount64() };
            if tween_progress(t, now).is_none() {
                self.caret_tween = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opt_out_never_tweens() {
        assert!(!should_tween(false, 0, 100, 5));
    }

    #[test]
    fn tweens_for_large_jump() {
        assert!(should_tween(true, 0, 10, 5));
    }

    #[test]
    fn no_tween_for_small_jump() {
        assert!(!should_tween(true, 10, 13, 5));
    }

    #[test]
    fn no_tween_at_exact_threshold() {
        assert!(!should_tween(true, 10, 15, 5));
        assert!(should_tween(true, 10, 16, 5));
    }

    #[test]
    fn tween_progress_starts_near_zero() {
        let t = CaretTween {
            from_line: 0,
            to_line: 10,
            started_ms: 1000,
            duration_ms: 160,
        };
        let p = tween_progress(t, 1000).expect("alive");
        assert!(p < 0.05);
    }

    #[test]
    fn tween_progress_eases_out_at_midpoint() {
        let t = CaretTween {
            from_line: 0,
            to_line: 10,
            started_ms: 1000,
            duration_ms: 160,
        };
        let p = tween_progress(t, 1080).expect("alive");
        assert!((p - 0.875).abs() < 0.01);
    }

    #[test]
    fn tween_progress_completes() {
        let t = CaretTween {
            from_line: 0,
            to_line: 10,
            started_ms: 1000,
            duration_ms: 160,
        };
        assert!(tween_progress(t, 1160).is_none());
        assert!(tween_progress(t, 9999).is_none());
    }

    #[test]
    fn tween_zero_duration_evicts_immediately() {
        let t = CaretTween {
            from_line: 0,
            to_line: 10,
            started_ms: 1000,
            duration_ms: 0,
        };
        assert!(tween_progress(t, 1000).is_none());
    }

    #[test]
    fn reduced_motion_policy_schedules_no_caret_frames() {
        assert!(crate::motion::MotionPolicy::new(true)
            .schedule(0, crate::motion::STRUCTURAL_MOTION_MS)
            .is_none());
    }
}
