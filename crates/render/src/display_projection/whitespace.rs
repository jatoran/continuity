//! Leading-whitespace DIP advance for soft-wrap continuation indent.
//! Pure rope query — no display map required — but lives on
//! [`FrameDisplay`] so soft-wrap callers reach it through the same type
//! they reach the rest of the projection API.

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
        if source_line >= rope.len_lines() {
            return 0.0;
        }
        let start = rope.line_to_byte(source_line);
        let next = if source_line + 1 < rope.len_lines() {
            rope.line_to_byte(source_line + 1)
        } else {
            rope.len_bytes()
        };
        let slice = rope.byte_slice(start..next).to_string();
        let mut advance = 0.0_f32;
        for ch in slice.chars() {
            match ch {
                ' ' => advance += column_advance,
                '\t' => advance += tab_advance,
                _ => break,
            }
        }
        advance
    }
}
