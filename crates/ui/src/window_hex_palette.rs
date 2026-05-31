//! Phase F3 — `HexInputMode` palette mode + Window opener stub.
//!
//! `HexInputMode` implements [`crate::palette_mode::PaletteMode`] for a
//! hex-only text-input prompt:
//!
//! - Accepts only `[0-9a-fA-F]` characters; non-hex input filters away.
//! - Commit is gated on the digit count being 3, 4, 6, or 8 (the four
//!   forms `parse_hex_rgba` understands).
//! - On commit, the mode dispatches `markdown.color_selection` with
//!   `{"hex": "<digits>"}` so the rope mutation path is identical to a
//!   pre-supplied JSON arg.
//!
//! The picker UI surface is the same single-line palette overlay used
//! for command palette / quick-open — the host overlay infrastructure
//! decides when to call [`crate::palette_mode::PaletteSession::set_query`]
//! and [`crate::palette_mode::PaletteSession::commit`]. Until the F3
//! overlay-wiring sub-wire lands, [`Window::open_hex_input_palette_impl`]
//! just records the request as a banner so the user knows to retry with
//! `markdown.color_selection {"hex":"..."}`.

use std::cell::RefCell;
use std::rc::Rc;

use continuity_decorate::parse_hex_rgba;
use serde_json::Value;

use crate::palette_mode::{PaletteMode, PaletteRow};
use crate::window::Window;
use crate::Error;

/// Bundle handed back to the host once the user commits a valid hex.
///
/// The host's commit listener pulls this out of [`HexInputMode::committed`]
/// after [`crate::palette_mode::PaletteSession::commit`] returns and
/// re-dispatches `markdown.color_selection` with the carried hex string.
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct HexCommit {
    /// The trimmed hex digits the user submitted (no leading `#`).
    pub digits: String,
}

/// A palette mode that accepts only hexadecimal digits and gates commit
/// on the 3 / 4 / 6 / 8-digit length contract. The mode keeps the
/// in-flight digits in a shared `RefCell` so the host can both push
/// keystrokes (via `set_query`) and read the committed value back out.
///
/// `#[allow(dead_code)]` is held against the per-method warning because
/// the overlay-side host wiring (which constructs the mode + plumbs it
/// through `PaletteSession`) lands in a follow-up sub-wire — the data
/// layer is intentionally shipped first so it's unit-tested before the
/// overlay integration.
#[allow(dead_code)]
pub struct HexInputMode {
    /// Trimmed running digits. Stored in a `RefCell` so `commit` (which
    /// borrows `&mut self`) and `filter` (which borrows `&self`) can both
    /// read/mutate it.
    digits: RefCell<String>,
    /// Filled in when commit succeeds; consumed by the host.
    committed: Rc<RefCell<Option<HexCommit>>>,
}

#[allow(dead_code)]
impl HexInputMode {
    /// Construct a new mode seeded with `prefill` (e.g. the last picked
    /// color). Non-hex characters in `prefill` are dropped silently.
    #[must_use]
    pub fn new(prefill: Option<&str>) -> Self {
        let seed = prefill
            .unwrap_or("")
            .trim_start_matches('#')
            .chars()
            .filter(|c| c.is_ascii_hexdigit())
            .take(8)
            .collect::<String>();
        Self {
            digits: RefCell::new(seed),
            committed: Rc::new(RefCell::new(None)),
        }
    }

    /// Shared handle for the host to pull out the committed hex once
    /// [`PaletteMode::commit`] returns `Ok(())`.
    #[must_use]
    pub fn committed_handle(&self) -> Rc<RefCell<Option<HexCommit>>> {
        Rc::clone(&self.committed)
    }

    /// Current trimmed digit count. Used by the host preview row.
    #[must_use]
    pub fn digit_count(&self) -> usize {
        self.digits.borrow().len()
    }

    /// `true` when the current digits are 3 / 4 / 6 / 8 characters long
    /// AND `parse_hex_rgba` returns `Some`.
    #[must_use]
    pub fn can_commit(&self) -> bool {
        let digits = self.digits.borrow();
        matches!(digits.len(), 3 | 4 | 6 | 8) && parse_hex_rgba(&digits).is_some()
    }
}

impl PaletteMode for HexInputMode {
    fn filter(&self, query: &str) -> Vec<PaletteRow> {
        // Each filter call accepts the host's input as authoritative.
        // We strip any leading `#` and discard anything that isn't a hex
        // digit so the row count and preview reflect only valid input.
        let cleaned: String = query
            .trim_start_matches('#')
            .chars()
            .filter(|c| c.is_ascii_hexdigit())
            .take(8)
            .collect();
        *self.digits.borrow_mut() = cleaned.clone();
        let preview_label = if cleaned.is_empty() {
            "type 3/4/6/8 hex digits".to_string()
        } else {
            format!("#{cleaned}")
        };
        let hint = match cleaned.len() {
            0 => "rgb / rgba / rrggbb / rrggbbaa".to_string(),
            3 | 4 | 6 | 8 => "Enter to apply".to_string(),
            other => format!("{other} digits — need 3/4/6/8"),
        };
        vec![PaletteRow::with_hint(preview_label, hint)]
    }

    fn commit(&mut self, _row: &PaletteRow) -> Result<(), Error> {
        if !self.can_commit() {
            return Err(Error::Command(continuity_command::Error::Other(format!(
                "hex input must be 3, 4, 6, or 8 digits ({} supplied)",
                self.digit_count()
            ))));
        }
        let digits = self.digits.borrow().clone();
        *self.committed.borrow_mut() = Some(HexCommit { digits });
        Ok(())
    }
}

impl Window {
    /// `markdown.color_selection` palette opener — switches the active
    /// overlay state to a [`crate::hex_picker::HexPicker`] seeded with
    /// `prefill`. The user types hex digits; Enter commits with the
    /// digits as the `{"hex": "..."}` JSON arg via
    /// [`crate::Window::confirm_hex_picker`]; Esc dismisses with no
    /// effect.
    pub(crate) fn open_hex_input_palette_impl(&mut self, prefill: Option<String>) {
        self.overlays.open_hex_picker(prefill.as_deref());
        self.focus_overlay_input();
        self.request_repaint();
    }

    /// Helper for tests / callers that already have a hex string ready:
    /// build the `Value` payload the `markdown.color_selection` handler
    /// expects.
    #[must_use]
    pub fn hex_args_value(hex: &str) -> Value {
        serde_json::json!({ "hex": hex })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette_mode::PaletteSession;

    #[test]
    fn empty_input_renders_help_row() {
        let mode = HexInputMode::new(None);
        let rows = mode.filter("");
        assert_eq!(rows.len(), 1);
        assert!(rows[0].hint.as_deref().unwrap().contains("rgb"));
    }

    #[test]
    fn invalid_chars_filtered_out() {
        let mode = HexInputMode::new(None);
        let rows = mode.filter("z!@#1g0");
        // `1` and `0` survive, the rest is discarded.
        assert_eq!(rows[0].label, "#10");
    }

    #[test]
    fn three_digits_commit_succeeds() {
        let mode = HexInputMode::new(None);
        let handle = mode.committed_handle();
        let mut session = PaletteSession::open(mode, "f06".to_string());
        session.commit().unwrap();
        let committed = handle.borrow().clone().unwrap();
        assert_eq!(committed.digits, "f06");
    }

    #[test]
    fn five_digits_commit_errors() {
        let mode = HexInputMode::new(None);
        let mut session = PaletteSession::open(mode, "fffff".to_string());
        let err = session.commit().unwrap_err();
        assert!(matches!(err, Error::Command(_)));
    }

    #[test]
    fn prefill_is_seeded() {
        let mode = HexInputMode::new(Some("f06a"));
        assert_eq!(mode.digit_count(), 4);
        assert!(mode.can_commit());
    }

    #[test]
    fn prefill_strips_leading_hash_and_invalid() {
        let mode = HexInputMode::new(Some("#zz12ab"));
        assert_eq!(mode.digit_count(), 4);
    }

    #[test]
    fn eight_digit_with_alpha_commits() {
        let mode = HexInputMode::new(None);
        let handle = mode.committed_handle();
        let mut session = PaletteSession::open(mode, "ff006688".to_string());
        session.commit().unwrap();
        assert_eq!(handle.borrow().as_ref().unwrap().digits, "ff006688");
    }

    #[test]
    fn hex_args_value_shape() {
        let v = Window::hex_args_value("f06");
        assert_eq!(v, serde_json::json!({ "hex": "f06" }));
    }
}
