//! Bidi (Unicode Bidirectional Algorithm) helpers.
//!
//! Cursor motion in this editor operates in **logical** byte order
//! always. The bidi level of a position is consulted only by visual
//! affordances (e.g., caret rendering hints, selection rectangles when
//! the layout has reversed visual runs). This module exposes the
//! minimum surface needed by those callers without dragging
//! `unicode-bidi` into every crate.
//!
//! See spec §12: "Bidi text rendering for RTL scripts" and §9 cursor
//! motion semantics.

use unicode_bidi::{BidiInfo, ParagraphInfo};

/// Bidi paragraph direction inferred from a string's strongly-typed
/// characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParagraphDirection {
    /// Left-to-right paragraph base direction.
    LeftToRight,
    /// Right-to-left paragraph base direction.
    RightToLeft,
    /// No strongly-typed characters; default to LTR per UAX #9.
    Neutral,
}

/// Compute the paragraph direction of `text`. Empty / pure-neutral
/// strings return [`ParagraphDirection::Neutral`].
#[must_use]
pub fn paragraph_direction(text: &str) -> ParagraphDirection {
    let info = BidiInfo::new(text, None);
    if info.paragraphs.is_empty() {
        return ParagraphDirection::Neutral;
    }
    let p: &ParagraphInfo = &info.paragraphs[0];
    let level = p.level;
    if level.is_rtl() {
        ParagraphDirection::RightToLeft
    } else if has_strong_ltr(text) {
        ParagraphDirection::LeftToRight
    } else {
        ParagraphDirection::Neutral
    }
}

/// Bidi level (0..=126) of the byte at `byte_offset` in `text`. Returns
/// `None` when the offset is out of bounds. Level 0 means LTR;
/// odd levels are RTL runs.
#[must_use]
pub fn level_at(text: &str, byte_offset: usize) -> Option<u8> {
    if byte_offset > text.len() {
        return None;
    }
    let info = BidiInfo::new(text, None);
    info.levels.get(byte_offset).map(|l| l.number())
}

fn has_strong_ltr(text: &str) -> bool {
    text.chars().any(|c| {
        let class = unicode_bidi::bidi_class(c);
        matches!(class, unicode_bidi::BidiClass::L)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_is_neutral() {
        assert_eq!(paragraph_direction(""), ParagraphDirection::Neutral);
    }

    #[test]
    fn pure_ascii_is_ltr() {
        assert_eq!(
            paragraph_direction("hello"),
            ParagraphDirection::LeftToRight
        );
    }

    #[test]
    fn arabic_paragraph_is_rtl() {
        // "Hello" in Arabic ("مرحبا").
        assert_eq!(
            paragraph_direction("مرحبا"),
            ParagraphDirection::RightToLeft
        );
    }

    #[test]
    fn level_at_returns_run_level() {
        let text = "hello مرحبا";
        // ASCII prefix is level 0 (LTR).
        assert_eq!(level_at(text, 0), Some(0));
        // Arabic suffix is level 1 (RTL run).
        let arabic_byte = text.find('م').unwrap();
        let level = level_at(text, arabic_byte).unwrap();
        assert!(level % 2 == 1, "expected RTL level, got {level}");
    }

    #[test]
    fn out_of_bounds_returns_none() {
        assert_eq!(level_at("abc", 999), None);
    }
}
