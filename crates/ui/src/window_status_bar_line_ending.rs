//! Line-ending detection for the status bar's encoding segment and
//! the Phase C3 mixed-line-ending warning chip.
//!
//! Split out of `window_status_bar.rs` to keep that file under the
//! 600-line cap (CLAUDE.md conventions).
//!
//! Thread ownership: pure functions, callable from any thread.

use ropey::Rope;

/// Phase C2 / C3 — detected line-ending convention of a rope. The
/// scanner samples up to [`LINE_ENDING_SCAN_BYTES`] bytes (cheap on a
/// 50 MB buffer); the result feeds both the encoding segment label
/// and the C3 mixed-LE warning chip detector.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum LineEnding {
    /// `\n`.
    Lf,
    /// `\r\n`.
    Crlf,
    /// Bare `\r`.
    Cr,
    /// More than one of the above appears in the scanned window.
    Mixed,
    /// No line breaks were observed.
    None,
}

impl LineEnding {
    /// Human-readable label for the status bar segment.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Lf => "LF",
            Self::Crlf => "CRLF",
            Self::Cr => "CR",
            Self::Mixed => "Mixed",
            Self::None => "—",
        }
    }

    /// `true` when more than one convention was observed.
    pub(crate) fn is_mixed(self) -> bool {
        matches!(self, Self::Mixed)
    }
}

/// Cap on the byte window the line-ending scanner inspects. Sized to
/// keep detection well under one millisecond on a 50 MB buffer.
pub(crate) const LINE_ENDING_SCAN_BYTES: usize = 65_536;

/// Scan up to [`LINE_ENDING_SCAN_BYTES`] of `rope` and classify the
/// observed line-ending convention.
pub(crate) fn detect_line_endings(rope: &Rope) -> LineEnding {
    let mut saw_lf = false;
    let mut saw_crlf = false;
    let mut saw_cr = false;
    let mut scanned = 0usize;
    let total = rope.len_bytes();
    let limit = LINE_ENDING_SCAN_BYTES.min(total);
    let mut prev_was_cr = false;
    for chunk in rope.chunks() {
        for &b in chunk.as_bytes() {
            scanned += 1;
            if scanned > limit {
                break;
            }
            match b {
                b'\n' => {
                    if prev_was_cr {
                        saw_crlf = true;
                    } else {
                        saw_lf = true;
                    }
                    prev_was_cr = false;
                }
                b'\r' => {
                    if prev_was_cr {
                        // Two CRs in a row — the first was a bare CR.
                        saw_cr = true;
                    }
                    prev_was_cr = true;
                }
                _ => {
                    if prev_was_cr {
                        saw_cr = true;
                    }
                    prev_was_cr = false;
                }
            }
        }
        if scanned >= limit {
            break;
        }
    }
    if prev_was_cr {
        saw_cr = true;
    }
    let kinds = u32::from(saw_lf) + u32::from(saw_crlf) + u32::from(saw_cr);
    match (kinds, saw_lf, saw_crlf, saw_cr) {
        (0, _, _, _) => LineEnding::None,
        (1, true, _, _) => LineEnding::Lf,
        (1, _, true, _) => LineEnding::Crlf,
        (1, _, _, true) => LineEnding::Cr,
        _ => LineEnding::Mixed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_line_endings_pure_lf() {
        let r = Rope::from_str("a\nb\nc\n");
        assert_eq!(detect_line_endings(&r), LineEnding::Lf);
    }

    #[test]
    fn detect_line_endings_pure_crlf() {
        let r = Rope::from_str("a\r\nb\r\nc\r\n");
        assert_eq!(detect_line_endings(&r), LineEnding::Crlf);
    }

    #[test]
    fn detect_line_endings_mixed() {
        let r = Rope::from_str("a\nb\r\nc\n");
        assert_eq!(detect_line_endings(&r), LineEnding::Mixed);
    }

    #[test]
    fn detect_line_endings_no_breaks() {
        let r = Rope::from_str("just one line");
        assert_eq!(detect_line_endings(&r), LineEnding::None);
    }
}
