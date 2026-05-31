//! Phase C3 — mixed-line-ending / mixed-indent warning chip detection.
//!
//! The detector runs at decoration-pass cadence (whatever produces a
//! status-bar repaint). It scans a bounded window of bytes from the
//! head of the rope, classifies the line-ending and indent conventions
//! it finds, and emits one chip per anomaly.
//!
//! Click handling lives in `window_mouse.rs`: clicking a chip dispatches
//! the corresponding normalize command, one undo group per normalize.
//!
//! Thread ownership: UI thread of one window. Called from
//! `Window::build_status_bar` and from `Window::dispatch_status_bar_click`.

use continuity_render::{StatusBarSegmentDraw, StatusBarSegmentKind};
use ropey::Rope;

use crate::window_status_bar_line_ending::{detect_line_endings, LINE_ENDING_SCAN_BYTES};

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
    if detect_indent_mixed(rope) {
        chips.push(StatusBarSegmentDraw {
            text: "Mixed indent".into(),
            kind: StatusBarSegmentKind::Chip,
            hover: Some("Normalize indentation".into()),
            alpha: 1.0,
        });
    }
    chips
}

/// Walk every line in the first [`LINE_ENDING_SCAN_BYTES`] of `rope`;
/// return `true` if at least one indented line uses spaces AND at
/// least one uses tabs. Lines whose indentation byte sequence is
/// length-zero or pure-not-whitespace are skipped.
fn detect_indent_mixed(rope: &Rope) -> bool {
    let mut saw_tab = false;
    let mut saw_space = false;
    let mut scanned = 0usize;
    let limit = LINE_ENDING_SCAN_BYTES.min(rope.len_bytes());
    for i in 0..rope.len_lines() {
        let line_start = rope.line_to_byte(i);
        if line_start >= limit {
            break;
        }
        let next_start = if i + 1 < rope.len_lines() {
            rope.line_to_byte(i + 1)
        } else {
            rope.len_bytes()
        };
        // Inspect only the leading whitespace of the line; bail at
        // the first non-WS byte.
        let mut byte_idx = line_start;
        while byte_idx < next_start && byte_idx < limit {
            let b = rope.byte(byte_idx);
            match b {
                b' ' => {
                    saw_space = true;
                    byte_idx += 1;
                }
                b'\t' => {
                    saw_tab = true;
                    byte_idx += 1;
                }
                _ => break,
            }
            scanned += 1;
            if saw_tab && saw_space {
                return true;
            }
        }
        if scanned >= limit {
            break;
        }
    }
    false
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
    fn pure_indent_no_chip() {
        let r = Rope::from_str("    line one\n    line two\n");
        assert!(!detect_indent_mixed(&r));
        let r = Rope::from_str("\tline one\n\tline two\n");
        assert!(!detect_indent_mixed(&r));
    }

    #[test]
    fn mixed_indent_emits_chip() {
        let r = Rope::from_str("    a\n\tb\n");
        assert!(detect_indent_mixed(&r));
        let chips = detect_chips(&r);
        assert!(chips.iter().any(|c| c.text.contains("indent")));
    }

    #[test]
    fn both_anomalies_emit_two_chips() {
        let r = Rope::from_str("    a\r\n\tb\n");
        let chips = detect_chips(&r);
        assert_eq!(chips.len(), 2);
    }

    #[test]
    fn unindented_lines_do_not_trigger_indent_chip() {
        let r = Rope::from_str("plain line\nplain line\n");
        assert!(!detect_indent_mixed(&r));
    }
}
