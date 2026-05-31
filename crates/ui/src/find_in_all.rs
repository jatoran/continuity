//! Find-in-all-buffers panel state.
//!
//! Holds the query, modes, and per-buffer match groups. Result rows are
//! computed by [`continuity_search::find_match_ranges`] over each open
//! buffer's snapshot. Rows are flattened into `flat_rows` for keyboard
//! navigation; the buffer-grouped layout is reconstructed at render time.

use continuity_buffer::BufferId;

use crate::text_input::TextInput;

/// One match row in the find-in-all panel.
#[derive(Clone, Debug)]
pub struct FlatRow {
    /// Buffer this row belongs to.
    pub buffer_id: BufferId,
    /// Buffer's display title.
    pub buffer_title: String,
    /// 1-indexed line number.
    pub line: u64,
    /// Inclusive start byte in the buffer.
    pub start_byte: usize,
    /// Exclusive end byte in the buffer.
    pub end_byte: usize,
    /// The full text of the matching line, for context display.
    pub line_text: String,
}

/// Find-in-all-buffers state.
#[derive(Debug, Default)]
pub struct FindInAll {
    /// Query input.
    pub input: TextInput,
    /// `true` for case-sensitive matching.
    pub case_sensitive: bool,
    /// `true` for whole-word matching.
    pub whole_word: bool,
    /// `true` to interpret the query as a regex.
    pub regex: bool,
    /// All flattened match rows in result order.
    pub flat_rows: Vec<FlatRow>,
    /// Currently-highlighted row.
    pub selected: usize,
}

impl FindInAll {
    /// A fresh panel.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the row list.
    pub(crate) fn set_rows(&mut self, rows: Vec<FlatRow>) {
        self.flat_rows = rows;
        self.selected = self.selected.min(self.flat_rows.len().saturating_sub(1));
    }

    /// Step the selection by `delta` rows.
    pub fn step(&mut self, delta: i32) {
        if self.flat_rows.is_empty() {
            self.selected = 0;
            return;
        }
        let len = self.flat_rows.len() as i32;
        let next = (self.selected as i32 + delta).max(0).min(len - 1);
        self.selected = next as usize;
    }

    /// Currently-selected row.
    #[must_use]
    pub(crate) fn selected_row(&self) -> Option<&FlatRow> {
        self.flat_rows.get(self.selected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(buf: BufferId, line: u64) -> FlatRow {
        FlatRow {
            buffer_id: buf,
            buffer_title: "x".into(),
            line,
            start_byte: 0,
            end_byte: 1,
            line_text: "x".into(),
        }
    }

    #[test]
    fn step_clamps() {
        let mut f = FindInAll::new();
        let id = BufferId::new();
        f.set_rows(vec![row(id, 1), row(id, 2), row(id, 3)]);
        f.step(-5);
        assert_eq!(f.selected, 0);
        f.step(50);
        assert_eq!(f.selected, 2);
    }

    #[test]
    fn selected_row_when_empty_is_none() {
        let f = FindInAll::new();
        assert!(f.selected_row().is_none());
    }
}
