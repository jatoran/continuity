//! Leading-whitespace / hanging-indent DIP advance for soft-wrap
//! continuation rows. Pure rope queries — no display map required — but
//! they live on [`FrameDisplay`] so soft-wrap callers reach them through
//! the same type they reach the rest of the projection API.

use ropey::Rope;

use super::FrameDisplay;

impl FrameDisplay {
    /// Rendered DIP advance of the leading whitespace on `source_line`.
    /// Each leading `' '` contributes `column_advance`; each leading
    /// `'\t'` contributes `tab_advance` (DirectWrite's resolved tab-stop
    /// width). Used to align soft-wrap continuation rows under the
    /// source line's first non-whitespace glyph regardless of whether
    /// the indent is tabs or spaces — without this, tab-indented lines
    /// collapse to a single space-width of hanging indent per tab.
    #[must_use]
    pub fn leading_whitespace_advance_dip(
        rope: &Rope,
        source_line: usize,
        column_advance: f32,
        tab_advance: f32,
    ) -> f32 {
        let Some(slice) = line_slice(rope, source_line) else {
            return 0.0;
        };
        leading_whitespace_advance(&slice, column_advance, tab_advance).0
    }

    /// Rendered DIP advance of the soft-wrap hanging indent for
    /// `source_line`: the leading whitespace plus, when the line is a
    /// list item, the rendered width of its list marker. Continuation
    /// rows of `- some long item …` align under the item content, not
    /// under the bullet glyph.
    ///
    /// The marker advance is approximated as one `column_advance` per
    /// rendered marker char (`• ` for unordered markers, the literal
    /// `N. ` / `N) ` for ordered ones). The approximation is
    /// deliberate: it keeps the indent independent of the caret-reveal
    /// state, so wrapped rows never shift horizontally as the caret
    /// enters or leaves the line.
    ///
    /// The result is clamped to
    /// [`continuity_display_map::wrap::MAX_HANG_INDENT_FRACTION`] of
    /// `available_text_width_dip` — the same cap the soft-wrap budget
    /// applies via `continuation_wrap_budget_dip` — so a deeply
    /// indented line keeps its painted right edge inside the text
    /// column instead of overflowing by the indent. Pass
    /// `f32::INFINITY` to disable the clamp (hit-test paths that
    /// mirror an unclamped historical offset must not — keep every
    /// caller on the same available width as its painter).
    #[must_use]
    pub fn hanging_indent_advance_dip(
        rope: &Rope,
        source_line: usize,
        column_advance: f32,
        tab_advance: f32,
        available_text_width_dip: f32,
    ) -> f32 {
        let Some(slice) = line_slice(rope, source_line) else {
            return 0.0;
        };
        let (whitespace_advance, after_indent) =
            leading_whitespace_advance(&slice, column_advance, tab_advance);
        let marker_columns =
            continuity_display_map::wrap::list_marker_display_columns(&slice[after_indent..]);
        let indent = whitespace_advance + marker_columns as f32 * column_advance;
        if available_text_width_dip.is_finite() && available_text_width_dip > 0.0 {
            indent.min(
                available_text_width_dip * continuity_display_map::wrap::MAX_HANG_INDENT_FRACTION,
            )
        } else {
            indent
        }
    }
}

fn line_slice(rope: &Rope, source_line: usize) -> Option<String> {
    if source_line >= rope.len_lines() {
        return None;
    }
    let start = rope.line_to_byte(source_line);
    let next = if source_line + 1 < rope.len_lines() {
        rope.line_to_byte(source_line + 1)
    } else {
        rope.len_bytes()
    };
    Some(rope.byte_slice(start..next).to_string())
}

/// Sum the leading-whitespace advance; also return the byte index of the
/// first non-whitespace char.
fn leading_whitespace_advance(slice: &str, column_advance: f32, tab_advance: f32) -> (f32, usize) {
    let mut advance = 0.0_f32;
    let mut idx = 0usize;
    for ch in slice.chars() {
        match ch {
            ' ' => advance += column_advance,
            '\t' => advance += tab_advance,
            _ => break,
        }
        idx += ch.len_utf8();
    }
    (advance, idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hanging_indent_adds_marker_width_to_whitespace() {
        let rope = Rope::from_str("  - a very long bullet item\nplain");
        let ws = FrameDisplay::leading_whitespace_advance_dip(&rope, 0, 8.0, 32.0);
        assert_eq!(ws, 16.0);
        let hang = FrameDisplay::hanging_indent_advance_dip(&rope, 0, 8.0, 32.0, 1000.0);
        assert_eq!(hang, 16.0 + 2.0 * 8.0);
        // Plain line: hanging indent == whitespace indent.
        let plain = FrameDisplay::hanging_indent_advance_dip(&rope, 1, 8.0, 32.0, 1000.0);
        assert_eq!(plain, 0.0);
    }

    #[test]
    fn hanging_indent_counts_ordered_marker_digits() {
        let rope = Rope::from_str("10. ordered item");
        let hang = FrameDisplay::hanging_indent_advance_dip(&rope, 0, 8.0, 32.0, 1000.0);
        assert_eq!(hang, 4.0 * 8.0);
    }

    #[test]
    fn tab_indented_bullet_uses_tab_advance_plus_marker() {
        let rope = Rope::from_str("\t- item");
        let hang = FrameDisplay::hanging_indent_advance_dip(&rope, 0, 8.0, 32.0, 1000.0);
        assert_eq!(hang, 32.0 + 16.0);
    }

    #[test]
    fn hanging_indent_clamps_to_fraction_of_available_width() {
        // Eight tabs at 32 DIPs = 256 DIPs of indent against a 100-DIP
        // column: the painter must cap at 75 so the continuation row
        // keeps a quarter of the column (matching the wrap budget's
        // floor) instead of painting past the right edge.
        let rope = Rope::from_str("\t\t\t\t\t\t\t\tdeep");
        let hang = FrameDisplay::hanging_indent_advance_dip(&rope, 0, 8.0, 32.0, 100.0);
        assert_eq!(hang, 75.0);
    }
}
