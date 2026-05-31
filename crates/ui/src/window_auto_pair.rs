//! Auto-pair glue between [`crate::Window`] and
//! [`continuity_core::edit_pairs`].
//!
//! The auto-pair *planner* lives in `core::edit_pairs`; this module
//! handles the per-window decision making that has to read settings
//! and the active rope:
//!
//! * [`Window::try_delete_auto_pair`] — backspace-aware delete:
//!   if every active caret sits between an empty configured pair,
//!   dispatch [`continuity_core::SelectionEdit::DeletePair`]; otherwise
//!   return `Ok(false)` so the caller falls through to plain
//!   [`continuity_command::Context::delete_back`].
//! * [`Window::apply_auto_pair_settings`] — copy the validated
//!   `[editor].auto_pair_*` toggles from a [`continuity_config::Settings`]
//!   onto the window's stored [`continuity_core::AutoPairConfig`].
//!
//! Thread ownership: every entry point runs on the window's UI thread.

use continuity_command::Error as CommandError; // alias: collides with crate::Error
use continuity_config::Settings;
use continuity_core::{AutoPairConfig, SelectionEdit};

use crate::Window;

impl Window {
    /// See module docs.
    pub(crate) fn try_delete_auto_pair(&mut self) -> Result<bool, CommandError> {
        // Single-caret-only fast path. Multi-caret with mixed pair
        // contents falls through to plain backspace; the planner would
        // otherwise refuse the whole group when one caret doesn't match,
        // which is the desired conservative behavior either way.
        let snap = match self.editor.snapshot(self.buffer_id) {
            Some(s) => s,
            None => return Ok(false),
        };
        let selections = snap.selections();
        if selections.len() != 1 {
            return Ok(false);
        }
        let sel = selections[0];
        if !sel.is_caret() {
            return Ok(false);
        }
        let rope = snap.rope_snapshot().rope();
        let line = sel.head.line as usize;
        let line_start = if line < rope.len_lines() {
            rope.line_to_byte(line)
        } else {
            rope.len_bytes()
        };
        let caret_byte = line_start + sel.head.byte_in_line as usize;
        let Some(prev) = char_before_byte(rope, caret_byte) else {
            return Ok(false);
        };
        let Some(next) = char_at_byte(rope, caret_byte) else {
            return Ok(false);
        };
        // The pair must be one this window has auto-pair enabled for —
        // otherwise the user typed `()` themselves and probably wants
        // backspace to delete just one char.
        let Some((expected_open, expected_close)) = self.auto_pair.pair_for(prev) else {
            return Ok(false);
        };
        if next != expected_close || prev != expected_open {
            return Ok(false);
        }
        self.dispatch_selection_edit(SelectionEdit::DeletePair {
            open: expected_open,
            close: expected_close,
        })?;
        Ok(true)
    }

    /// Mirror the validated `[editor].auto_pair_*` toggles onto the
    /// window's [`AutoPairConfig`]. Idempotent.
    pub(crate) fn apply_auto_pair_settings(&mut self, s: &Settings) {
        self.auto_pair = AutoPairConfig {
            paren: s.editor.auto_pair_paren,
            bracket: s.editor.auto_pair_bracket,
            brace: s.editor.auto_pair_brace,
            dquote: s.editor.auto_pair_dquote,
            squote: s.editor.auto_pair_squote,
            backtick: s.editor.auto_pair_backtick,
            asterisk: s.editor.auto_pair_asterisk,
            underscore: s.editor.auto_pair_underscore,
        };
    }
}

fn char_before_byte(rope: &ropey::Rope, byte: usize) -> Option<char> {
    if byte == 0 || byte > rope.len_bytes() {
        return None;
    }
    let char_idx_at = rope.byte_to_char(byte);
    if char_idx_at == 0 {
        return None;
    }
    rope.chars_at(char_idx_at - 1).next()
}

fn char_at_byte(rope: &ropey::Rope, byte: usize) -> Option<char> {
    if byte >= rope.len_bytes() {
        return None;
    }
    let char_idx = rope.byte_to_char(byte);
    rope.chars_at(char_idx).next()
}
