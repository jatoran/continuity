//! UI-thread state for the passive footnote hover-peek.
//!
//! The state is stored on [`crate::mouse::MouseState`], whose owner is the
//! window UI thread. It never crosses threads and never mutates buffer text.

use continuity_decorate::ByteRange;

/// Dwell before a hovered footnote reference surfaces the peek panel.
pub(crate) const FOOTNOTE_HOVER_DWELL_MS: u64 = 300;

/// In-flight or visible hover-peek state for one footnote reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FootnoteHover {
    /// Footnote label without delimiters.
    pub label: String,
    /// Source range of the hovered reference.
    pub reference_range: ByteRange,
    /// Formatted body text shown in the panel.
    pub body_text: String,
    /// Client x where the hover was last observed.
    pub anchor_x: i32,
    /// Client y where the hover was last observed.
    pub anchor_y: i32,
    /// Wall-clock ms when the cursor entered this reference.
    pub started_ms: u64,
    /// `true` after the dwell timer has fired while still over the reference.
    pub ready: bool,
}

impl FootnoteHover {
    /// `true` if `label` and `reference_range` identify the same source target.
    #[must_use]
    pub fn is_same_reference(&self, label: &str, reference_range: ByteRange) -> bool {
        self.label == label && self.reference_range == reference_range
    }

    /// `true` when `now_ms` has passed the dwell threshold.
    #[must_use]
    pub fn dwell_elapsed(&self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.started_ms) >= FOOTNOTE_HOVER_DWELL_MS
    }
}

/// Clean source text for a compact peek panel.
#[must_use]
pub(crate) fn format_footnote_body(raw: &str) -> String {
    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<String> = normalized
        .lines()
        .map(|line| {
            line.strip_prefix("    ")
                .or_else(|| line.strip_prefix('\t'))
                .unwrap_or(line)
                .trim_end()
                .to_string()
        })
        .collect();
    lines.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_formatter_dedents_continuations() {
        let body = "first\n    second\n\tthird\n";
        assert_eq!(format_footnote_body(body), "first\nsecond\nthird");
    }

    #[test]
    fn dwell_uses_elapsed_ms() {
        let hover = FootnoteHover {
            label: "1".into(),
            reference_range: ByteRange::new(0, 4),
            body_text: "body".into(),
            anchor_x: 0,
            anchor_y: 0,
            started_ms: 1_000,
            ready: false,
        };
        assert!(!hover.dwell_elapsed(1_299));
        assert!(hover.dwell_elapsed(1_300));
    }
}
