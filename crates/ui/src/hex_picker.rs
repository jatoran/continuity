//! Phase F3 — hex-input overlay state.
//!
//! Single-input overlay that accepts only `[0-9a-fA-F]` characters and
//! commits on Enter when the digit count is 3, 4, 6, or 8. On commit
//! the host re-dispatches `markdown.color_selection` with the entered
//! digits as the `{"hex": "..."}` arg so the wrap path matches the
//! pre-supplied-arg shape exactly.

use crate::text_input::TextInput;

/// Hex-input overlay state. Held as the payload of
/// [`crate::Overlays::HexPicker`].
#[derive(Debug, Default)]
pub struct HexPicker {
    /// In-flight digits. Only `[0-9a-fA-F]` characters are accepted via
    /// [`Self::insert_char`]; everything else is silently dropped.
    pub input: TextInput,
}

impl HexPicker {
    /// Construct a fresh picker seeded with `prefill` (e.g. the last
    /// picked color). Non-hex characters and any leading `#` are
    /// stripped; longer inputs are clamped to 8 digits (the
    /// `rrggbbaa` cap).
    #[must_use]
    pub fn new(prefill: Option<&str>) -> Self {
        let mut input = TextInput::default();
        if let Some(p) = prefill {
            let cleaned: String = p
                .trim_start_matches('#')
                .chars()
                .filter(|c| c.is_ascii_hexdigit())
                .take(8)
                .collect();
            for ch in cleaned.chars() {
                input.insert_char(ch);
            }
        }
        Self { input }
    }

    /// Accept a typed character. Non-hex digits are silently dropped so
    /// the input field never holds garbage.
    pub fn insert_char(&mut self, ch: char) {
        if !ch.is_ascii_hexdigit() {
            return;
        }
        if self.input.text.len() >= 8 {
            return;
        }
        self.input.insert_char(ch);
    }

    /// `true` when the in-flight digits parse cleanly as a 3 / 4 / 6 / 8-
    /// digit hex value. Used by the host to gate commit on Enter and
    /// by the renderer to show a "ready" affordance.
    #[must_use]
    pub fn can_commit(&self) -> bool {
        matches!(self.input.text.len(), 3 | 4 | 6 | 8)
            && self.input.text.chars().all(|c| c.is_ascii_hexdigit())
    }

    /// Current digit count. Used by the host preview row.
    #[must_use]
    pub fn digit_count(&self) -> usize {
        self.input.text.len()
    }

    /// Borrow the trimmed digits so the host can build the JSON arg
    /// payload (`{"hex": "..."}`) without cloning.
    #[must_use]
    pub fn digits(&self) -> &str {
        &self.input.text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_cannot_commit() {
        let p = HexPicker::new(None);
        assert!(!p.can_commit());
        assert_eq!(p.digit_count(), 0);
    }

    #[test]
    fn invalid_chars_filtered_out() {
        let mut p = HexPicker::new(None);
        p.insert_char('z');
        p.insert_char('!');
        p.insert_char('1');
        p.insert_char('0');
        assert_eq!(p.digits(), "10");
    }

    #[test]
    fn three_digit_commit_ready() {
        let p = HexPicker::new(Some("f06"));
        assert!(p.can_commit());
        assert_eq!(p.digits(), "f06");
    }

    #[test]
    fn five_digit_blocks_commit() {
        let p = HexPicker::new(Some("fffff"));
        assert!(!p.can_commit());
    }

    #[test]
    fn prefill_strips_hash_and_invalid() {
        let p = HexPicker::new(Some("#zz12ab"));
        assert_eq!(p.digits(), "12ab");
        assert!(p.can_commit());
    }

    #[test]
    fn input_clamped_to_eight_digits() {
        let mut p = HexPicker::new(None);
        for c in "fffffffffff".chars() {
            p.insert_char(c);
        }
        assert_eq!(p.digit_count(), 8);
    }
}
