//! One-shot status-bar notices.
//!
//! Thread ownership: the `Window`'s UI thread owns the notice vector.
//! Notices are converted into the render crate's status-bar chip payload
//! during `Window::build_status_bar`; no separate widget system exists.

use continuity_render::{StatusBarSegmentDraw, StatusBarSegmentKind};

const NOTICE_LIFETIME_MS: u64 = 4_000;
const NOTICE_FADE_MS: u64 = 900;
/// α.1 save-confirm chip lives for ~1.8 s, well shorter than the
/// 4 s decoration-worker notice, since a save is a routine confirmation
/// rather than an actionable warning.
const SAVE_NOTICE_LIFETIME_MS: u64 = 1_800;
/// The save-confirm chip fades over its final ~400 ms so the disappearance
/// satisfies the motion contract's "value-change transient" rule.
const SAVE_NOTICE_FADE_MS: u64 = 400;

/// One transient notice shown in the right-side status-bar chip lane.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StatusNotice {
    text: String,
    _created_at_ms: u64,
    expires_at_ms: u64,
    fade_ms: u64,
}

impl StatusNotice {
    /// Build the decoration-worker restart notice.
    #[must_use]
    pub(crate) fn decoration_worker_restarted(now_ms: u64) -> Self {
        Self {
            text: "Decoration worker restarted".into(),
            _created_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(NOTICE_LIFETIME_MS),
            fade_ms: NOTICE_FADE_MS,
        }
    }

    /// α.1 save-confirm chip. Built when a file-associated buffer's
    /// bytes hit disk via `FileIoEvent::Saved`. The text is the
    /// formatted filename only (no path) — the banner already shows
    /// the full path; the chip is a glanceable acknowledgement.
    #[must_use]
    pub(crate) fn saved(file_name: &str, now_ms: u64) -> Self {
        Self {
            text: format!("Saved {file_name}"),
            _created_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(SAVE_NOTICE_LIFETIME_MS),
            fade_ms: SAVE_NOTICE_FADE_MS,
        }
    }

    fn alpha_at(&self, now_ms: u64, reduced_motion: bool) -> Option<f32> {
        if now_ms >= self.expires_at_ms {
            return None;
        }
        if reduced_motion {
            return Some(1.0);
        }
        let fade_start = self.expires_at_ms.saturating_sub(self.fade_ms);
        if now_ms <= fade_start {
            return Some(1.0);
        }
        let remaining = self.expires_at_ms.saturating_sub(now_ms);
        Some((remaining as f32 / self.fade_ms as f32).clamp(0.0, 1.0))
    }

    fn to_chip(&self, now_ms: u64, reduced_motion: bool) -> Option<StatusBarSegmentDraw> {
        let alpha = self.alpha_at(now_ms, reduced_motion)?;
        Some(StatusBarSegmentDraw {
            text: self.text.clone(),
            kind: StatusBarSegmentKind::NoticeChip,
            hover: None,
            alpha,
        })
    }
}

/// Add or refresh the decoration-worker restart notice.
pub(crate) fn push_decoration_restart_notice(notices: &mut Vec<StatusNotice>, now_ms: u64) {
    notices.retain(|notice| notice.text != "Decoration worker restarted");
    notices.push(StatusNotice::decoration_worker_restarted(now_ms));
}

/// α.1 — push a save-confirm chip for `file_name`. Replaces any
/// prior save chip so repeated rapid saves coalesce to a single live
/// chip instead of stacking.
pub(crate) fn push_save_confirm_notice(
    notices: &mut Vec<StatusNotice>,
    file_name: &str,
    now_ms: u64,
) {
    notices.retain(|notice| !notice.text.starts_with("Saved "));
    notices.push(StatusNotice::saved(file_name, now_ms));
}

/// Drop expired notices. Returns `true` when the vector changed.
pub(crate) fn retain_live_notices(notices: &mut Vec<StatusNotice>, now_ms: u64) -> bool {
    let before = notices.len();
    notices.retain(|notice| notice.alpha_at(now_ms, false).is_some());
    before != notices.len()
}

/// Append visible notices to the status-bar chip list.
pub(crate) fn append_notice_chips(
    chips: &mut Vec<StatusBarSegmentDraw>,
    notices: &[StatusNotice],
    now_ms: u64,
    reduced_motion: bool,
) {
    for notice in notices {
        if let Some(chip) = notice.to_chip(now_ms, reduced_motion) {
            chips.push(chip);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_render::{StatusBarColors, StatusBarData};

    #[test]
    fn restart_notice_becomes_status_bar_chip() {
        let notices = vec![StatusNotice::decoration_worker_restarted(1_000)];
        let mut chips = Vec::new();
        append_notice_chips(&mut chips, &notices, 1_000, false);

        assert_eq!(chips.len(), 1);
        assert_eq!(chips[0].text, "Decoration worker restarted");
        assert_eq!(chips[0].kind, StatusBarSegmentKind::NoticeChip);

        let data = StatusBarData {
            segments: &[],
            chips: &chips,
            colors: StatusBarColors::default(),
            transients: &[],
        };
        let layout = continuity_render::compute_status_bar_layout(&data, 800.0, 600.0, 14.0);
        assert_eq!(layout.bounds.len(), 1);
        assert_eq!(layout.bounds[0].kind, StatusBarSegmentKind::NoticeChip);
    }

    #[test]
    fn restart_notice_fades_then_expires() {
        let notice = StatusNotice::decoration_worker_restarted(10);
        assert_eq!(notice.alpha_at(10, false), Some(1.0));
        let fading = notice.alpha_at(10 + NOTICE_LIFETIME_MS - 100, false);
        assert!(matches!(fading, Some(alpha) if alpha < 1.0 && alpha > 0.0));
        assert_eq!(notice.alpha_at(10 + NOTICE_LIFETIME_MS, false), None);
    }

    #[test]
    fn reduced_motion_notice_does_not_fade() {
        let notice = StatusNotice::decoration_worker_restarted(10);
        assert_eq!(
            notice.alpha_at(10 + NOTICE_LIFETIME_MS - 100, true),
            Some(1.0)
        );
    }

    #[test]
    fn push_restart_notice_refreshes_existing_chip() {
        let mut notices = vec![StatusNotice::decoration_worker_restarted(1_000)];
        push_decoration_restart_notice(&mut notices, 2_000);

        assert_eq!(notices.len(), 1);
        assert!(notices[0]
            .alpha_at(2_000 + NOTICE_LIFETIME_MS - 100, false)
            .is_some());
    }

    #[test]
    fn save_confirm_chip_renders_filename() {
        let mut notices = Vec::new();
        push_save_confirm_notice(&mut notices, "notes.md", 1_000);
        let mut chips = Vec::new();
        append_notice_chips(&mut chips, &notices, 1_000, false);
        assert_eq!(chips.len(), 1);
        assert_eq!(chips[0].text, "Saved notes.md");
        assert_eq!(chips[0].kind, StatusBarSegmentKind::NoticeChip);
    }

    #[test]
    fn save_confirm_chip_expires_under_two_seconds() {
        let notice = StatusNotice::saved("notes.md", 0);
        assert_eq!(notice.alpha_at(0, false), Some(1.0));
        assert!(notice.alpha_at(SAVE_NOTICE_LIFETIME_MS, false).is_none());
    }

    #[test]
    fn save_confirm_chip_fades_in_final_window() {
        let notice = StatusNotice::saved("notes.md", 0);
        // Mid-fade (last 200 ms of life): alpha is in (0, 1).
        let alpha = notice
            .alpha_at(SAVE_NOTICE_LIFETIME_MS - 200, false)
            .expect("alive");
        assert!(alpha > 0.0 && alpha < 1.0, "alpha={alpha}");
    }

    #[test]
    fn pushing_save_chip_coalesces_repeat_saves() {
        let mut notices = Vec::new();
        push_save_confirm_notice(&mut notices, "notes.md", 1_000);
        push_save_confirm_notice(&mut notices, "notes.md", 1_500);
        assert_eq!(notices.len(), 1);
    }

    #[test]
    fn reduced_motion_save_chip_stays_solid() {
        let notice = StatusNotice::saved("notes.md", 0);
        assert_eq!(
            notice.alpha_at(SAVE_NOTICE_LIFETIME_MS - 50, true),
            Some(1.0)
        );
    }
}
