//! Mixed-line-ending warning-chip detection.
//!
//! The detector runs at decoration-pass cadence (whatever produces a
//! status-bar repaint). Indentation diagnostics deliberately stay out of
//! persistent chrome; conversion remains available through commands.
//!
//! Click handling lives in `window_mouse.rs`: clicking a chip dispatches
//! the corresponding normalize command, one undo group per normalize.
//!
//! Thread ownership: UI thread of one window. Called from
//! `Window::build_status_bar` and from `Window::dispatch_status_bar_click`.

use continuity_render::{StatusBarSegmentDraw, StatusBarSegmentKind};
use ropey::Rope;

use crate::window_status_bar_line_ending::detect_line_endings;

/// Build the list of warning chips for the current rope. Empty when no
/// anomaly is detected.
pub(crate) fn detect_chips(rope: &Rope) -> Vec<StatusBarSegmentDraw> {
    let mut chips: Vec<StatusBarSegmentDraw> = Vec::new();
    if detect_line_endings(rope).is_mixed() {
        chips.push(StatusBarSegmentDraw {
            text: "Mixed LE".into(),
            kind: StatusBarSegmentKind::Chip,
            hover: Some("Normalize line endings".into()),
            alpha: 1.0,
        });
    }
    chips
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_lf_no_chips() {
        let r = Rope::from_str("a\nb\nc\n");
        let chips = detect_chips(&r);
        assert!(chips.is_empty());
    }

    #[test]
    fn mixed_le_emits_chip() {
        let r = Rope::from_str("a\nb\r\nc\n");
        let chips = detect_chips(&r);
        assert_eq!(chips.len(), 1);
        assert!(chips[0].text.contains("LE"));
        assert_eq!(chips[0].kind, StatusBarSegmentKind::Chip);
    }

    #[test]
    fn mixed_indent_does_not_emit_status_chip() {
        let r = Rope::from_str("    a\n\tb\n");
        let chips = detect_chips(&r);
        assert!(chips.is_empty());
    }

    #[test]
    fn mixed_line_endings_remain_visible_when_indent_is_mixed() {
        let r = Rope::from_str("    a\r\n\tb\n");
        let chips = detect_chips(&r);
        assert_eq!(chips.len(), 1);
        assert_eq!(chips[0].text, "Mixed LE");
    }
}
