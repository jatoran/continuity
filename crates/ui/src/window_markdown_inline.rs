//! Phase F3 + F4 — `Window` impls for inline-color markup mutation and
//! pipe-table skeleton insertion.
//!
//! Each handler runs on the UI thread and funnels rope mutation through
//! [`continuity_core::EditorHandle::apply_edit`] so the change lands in
//! one [`continuity_text::UndoGroupId`]. Decoration recompute is driven
//! by the existing revision-keyed cache; no extra invalidation hooks
//! beyond [`crate::window_helpers::invalidate_hwnd`] are needed.

use continuity_decorate::{
    inline_color_spans, table_formula::format_table_skeleton, InlineColorKind,
};
use continuity_text::{EditOp, Position, Range};

use crate::window::Window;
use crate::window_helpers::invalidate_hwnd;
use crate::window_view_context::map_ui_to_command_error;

impl Window {
    /// `markdown.highlight_selection` impl — wrap the primary selection
    /// (or the caret position if empty) in `==…==`. One undo group.
    pub(crate) fn markdown_highlight_selection_impl(
        &mut self,
    ) -> Result<(), continuity_command::Error> {
        self.wrap_selection_with_delimiters("==", "==")
    }

    /// `markdown.color_selection` impl — wrap the primary selection in
    /// `{#hex:…}` when `prefill` is supplied; otherwise open the
    /// hex-input palette mode and wait for the user's commit.
    pub(crate) fn markdown_color_selection_impl(
        &mut self,
        prefill: Option<&str>,
    ) -> Result<(), continuity_command::Error> {
        match prefill {
            Some(hex) if is_valid_hex_digit_count(hex) => {
                let opener = format!("{{#{hex}:");
                self.wrap_selection_with_delimiters(&opener, "}")
            }
            _ => {
                // No / invalid prefill — open the hex-input palette mode.
                // The palette commit handler re-dispatches this command
                // with `{"hex": "..."}` once the user submits a valid
                // value. `open_hex_input_palette` lives in
                // `window_hex_palette.rs`.
                self.open_hex_input_palette_impl(prefill.map(str::to_string));
                Ok(())
            }
        }
    }

    /// `markdown.clear_inline_color` impl — unwrap whichever inline
    /// color/highlight span surrounds the primary caret. No-op when the
    /// caret is not inside any span.
    pub(crate) fn markdown_clear_inline_color_impl(
        &mut self,
    ) -> Result<(), continuity_command::Error> {
        let snap = match self.editor.snapshot(self.buffer_id) {
            Some(s) => s,
            None => return Ok(()),
        };
        let rope = snap.rope_snapshot().rope();
        let sels = snap.selections();
        let primary = match sels.first() {
            Some(s) => s,
            None => return Ok(()),
        };
        let caret_byte = position_to_byte(rope, primary.head);
        let source: String = rope.to_string();
        let spans = inline_color_spans(&source);
        let Some(span) = spans
            .iter()
            .find(|s| caret_byte >= s.outer.start && caret_byte < s.outer.end)
        else {
            return Ok(());
        };
        let inner_text = source.get(span.inner.clone()).unwrap_or("").to_string();
        let start_pos = byte_to_position(rope, span.outer.start);
        let end_pos = byte_to_position(rope, span.outer.end);
        // Discriminate Highlight vs Hex purely for the docstring narrative;
        // both branches replace `outer` with `inner` text.
        let _ = matches!(span.kind, InlineColorKind::Highlight);
        self.editor
            .apply_edit(
                self.buffer_id,
                EditOp::replace(Range::new(start_pos, end_pos), inner_text),
            )
            .map_err(|e| map_ui_to_command_error(crate::Error::Core(e)))?;
        self.cancel_display_prewarm_for_buffer(self.buffer_id);
        invalidate_hwnd(self.hwnd);
        Ok(())
    }

    /// `markdown.insert_table` impl — insert a column-aligned GFM table
    /// skeleton at the start of the caret line. One undo group. Caret
    /// moves to the first body cell so the user can type immediately.
    pub(crate) fn markdown_insert_table_impl(
        &mut self,
        rows: u32,
        cols: u32,
    ) -> Result<(), continuity_command::Error> {
        let rows = rows.max(1);
        let cols = cols.max(1);
        let snap = match self.editor.snapshot(self.buffer_id) {
            Some(s) => s,
            None => return Ok(()),
        };
        let body = format_table_skeleton(rows, cols);
        let caret_line = snap.selections().first().map(|s| s.head.line).unwrap_or(0);
        let insert_at = Position::new(caret_line, 0);
        self.editor
            .apply_edit(self.buffer_id, EditOp::insert(insert_at, body))
            .map_err(|e| map_ui_to_command_error(crate::Error::Core(e)))?;
        self.cancel_display_prewarm_for_buffer(self.buffer_id);
        invalidate_hwnd(self.hwnd);
        Ok(())
    }

    /// Wrap the primary selection range (or insert at the caret) with
    /// `prefix` / `suffix`. One undo group via `Range::replace`. When
    /// the selection is empty, both delimiters are dropped in adjacent
    /// to the caret and the user can type the body between them.
    fn wrap_selection_with_delimiters(
        &mut self,
        prefix: &str,
        suffix: &str,
    ) -> Result<(), continuity_command::Error> {
        let snap = match self.editor.snapshot(self.buffer_id) {
            Some(s) => s,
            None => return Ok(()),
        };
        let rope = snap.rope_snapshot().rope();
        let sels = snap.selections();
        let primary = match sels.first() {
            Some(s) => s,
            None => return Ok(()),
        };
        let ordered = primary.ordered_range();
        let start_byte = position_to_byte(rope, ordered.start);
        let end_byte = position_to_byte(rope, ordered.end);
        let source: String = rope.to_string();
        let inner = source.get(start_byte..end_byte).unwrap_or("").to_string();
        let wrapped = format!("{prefix}{inner}{suffix}");
        self.editor
            .apply_edit(
                self.buffer_id,
                EditOp::replace(Range::new(ordered.start, ordered.end), wrapped),
            )
            .map_err(|e| map_ui_to_command_error(crate::Error::Core(e)))?;
        self.cancel_display_prewarm_for_buffer(self.buffer_id);
        invalidate_hwnd(self.hwnd);
        Ok(())
    }
}

/// `true` when `hex` has 3, 4, 6, or 8 hexadecimal digits and no other
/// characters.
fn is_valid_hex_digit_count(hex: &str) -> bool {
    let digits = hex.trim_start_matches('#');
    matches!(digits.len(), 3 | 4 | 6 | 8) && digits.chars().all(|c| c.is_ascii_hexdigit())
}

/// Document-absolute UTF-8 byte offset of a `Position` in `rope`.
fn position_to_byte(rope: &ropey::Rope, pos: Position) -> usize {
    let line_start = rope.line_to_byte(pos.line as usize);
    line_start + pos.byte_in_line as usize
}

/// Inverse of [`position_to_byte`] — clamps past-EOF to the last byte.
fn byte_to_position(rope: &ropey::Rope, byte: usize) -> Position {
    let total = rope.len_bytes();
    let b = byte.min(total);
    let line = rope.byte_to_line(b);
    let line_start = rope.line_to_byte(line);
    Position::new(line as u32, (b - line_start) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_digit_counts_accepted() {
        for v in ["f06", "f06a", "ff0066", "ff0066aa"] {
            assert!(is_valid_hex_digit_count(v), "expected {v} to be valid");
        }
    }

    #[test]
    fn hex_digit_counts_rejected() {
        for v in ["", "f", "ff", "fffff", "fffffff", "zzz", "g00"] {
            assert!(!is_valid_hex_digit_count(v), "expected {v} to be invalid");
        }
    }

    #[test]
    fn hex_accepts_leading_hash() {
        assert!(is_valid_hex_digit_count("#f06"));
        assert!(is_valid_hex_digit_count("#ff0066"));
    }

    #[test]
    fn position_byte_round_trip() {
        let rope = ropey::Rope::from_str("line0\nline1\nline2");
        let pos = Position::new(1, 3);
        let byte = position_to_byte(&rope, pos);
        let recovered = byte_to_position(&rope, byte);
        assert_eq!(recovered, pos);
    }
}
