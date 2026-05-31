//! Cell-selection and cell-edge helpers for pipe-table commands.

use continuity_command::Error;
use continuity_decorate::EvaluatedTable;
use continuity_text::{Position, Selection, SelectionKind};

use crate::window::Window;
use crate::window_helpers::invalidate_hwnd;
use crate::window_markdown_table_ops::{
    col_index_for_caret, parse_table_rows, row_index_for_caret,
};
use crate::window_view_context::map_ui_to_command_error;

impl Window {
    /// Bound to Ctrl+A inside a table cell. For every selection whose
    /// head lies inside a cell's `source_range`, replace it with a
    /// selection that spans the cell's content (anchor at cell start,
    /// head at cell end). Carets outside any cell keep their current
    /// selection. Returns `UnsupportedContext` when NO caret was in a
    /// table cell so the keymap layer falls through to the global
    /// Ctrl+A.
    pub(crate) fn markdown_table_select_cell_impl(&mut self) -> Result<(), Error> {
        let snap = self
            .editor
            .snapshot(self.buffer_id)
            .ok_or(Error::UnsupportedContext("no buffer"))?;
        let rope = snap.rope_snapshot().rope();
        let id = self.buffer_id.as_uuid().as_u128();
        let dec = self
            .decoration_cache
            .get(id)
            .ok_or(Error::UnsupportedContext("no decorations"))?;
        let mut new_selections: Vec<Selection> = Vec::with_capacity(snap.selections().len());
        let mut any_in_cell = false;
        for sel in snap.selections() {
            let caret_byte = position_to_absolute_byte(rope, sel.head);
            if let Some(cell_range) = find_cell_at_caret(rope, &dec.evaluated_tables, caret_byte) {
                any_in_cell = true;
                let start_pos = absolute_byte_to_position(rope, cell_range.start);
                let end_pos = absolute_byte_to_position(rope, cell_range.end);
                new_selections.push(Selection::new(start_pos, end_pos, SelectionKind::Caret));
            } else {
                new_selections.push(*sel);
            }
        }
        if !any_in_cell {
            return Err(Error::UnsupportedContext("no caret in any table cell"));
        }
        self.editor
            .set_selections(self.buffer_id, new_selections)
            .map_err(|e| map_ui_to_command_error(crate::Error::Core(e)))?;
        invalidate_hwnd(self.hwnd);
        Ok(())
    }

    /// Bound to Home / Shift+Home / End / Shift+End inside a table
    /// cell. Sets each caret to its cell's content start (or end);
    /// `extend = true` keeps the existing anchor so the move grows
    /// the selection. Returns `UnsupportedContext` when NO caret is
    /// in a table cell so the keymap layer falls through to the
    /// global Home/End binding.
    pub(crate) fn markdown_table_caret_cell_edge_impl(
        &mut self,
        to_start: bool,
        extend: bool,
    ) -> Result<(), Error> {
        let snap = self
            .editor
            .snapshot(self.buffer_id)
            .ok_or(Error::UnsupportedContext("no buffer"))?;
        let rope = snap.rope_snapshot().rope();
        let id = self.buffer_id.as_uuid().as_u128();
        let dec = self
            .decoration_cache
            .get(id)
            .ok_or(Error::UnsupportedContext("no decorations"))?;
        let mut new_selections: Vec<Selection> = Vec::with_capacity(snap.selections().len());
        let mut any_in_cell = false;
        for sel in snap.selections() {
            let caret_byte = position_to_absolute_byte(rope, sel.head);
            let mut updated = *sel;
            if let Some(cell_range) = find_cell_at_caret(rope, &dec.evaluated_tables, caret_byte) {
                any_in_cell = true;
                let target_byte = if to_start {
                    cell_range.start
                } else {
                    cell_range.end
                };
                let target_pos = absolute_byte_to_position(rope, target_byte);
                updated.head = target_pos;
                if !extend {
                    updated.anchor = target_pos;
                }
                updated.kind = SelectionKind::Caret;
            }
            new_selections.push(updated);
        }
        if !any_in_cell {
            return Err(Error::UnsupportedContext("no caret in any table cell"));
        }
        self.editor
            .set_selections(self.buffer_id, new_selections)
            .map_err(|e| map_ui_to_command_error(crate::Error::Core(e)))?;
        invalidate_hwnd(self.hwnd);
        Ok(())
    }
}

fn absolute_byte_to_position(rope: &ropey::Rope, byte: usize) -> Position {
    let clamped = byte.min(rope.len_bytes());
    let line = rope.byte_to_line(clamped);
    let line_start = rope.line_to_byte(line);
    Position::new(line as u32, (clamped - line_start) as u32)
}

fn position_to_absolute_byte(rope: &ropey::Rope, pos: Position) -> usize {
    let line = pos.line as usize;
    let line_start = if line < rope.len_lines() {
        rope.line_to_byte(line)
    } else {
        rope.len_bytes()
    };
    line_start + pos.byte_in_line as usize
}

fn find_cell_at_caret(
    rope: &ropey::Rope,
    tables: &[EvaluatedTable],
    caret_byte: usize,
) -> Option<std::ops::Range<usize>> {
    let table = tables
        .iter()
        .find(|t| caret_byte >= t.block_range.start && caret_byte < t.block_range.end)?;
    let rows = parse_table_rows(rope, table);
    let row_idx = row_index_for_caret(&rows, caret_byte)?;
    let row = &rows[row_idx];
    if row.is_delimiter {
        return None;
    }
    let col = col_index_for_caret(row, caret_byte)?;
    let slot = row.cell_slot(col)?;
    let line_text: String = rope
        .byte_slice(row.line_start..row.line_end_with_newline)
        .to_string();
    let slot_local_start = slot.start - row.line_start;
    let slot_local_end = slot.end - row.line_start;
    let bytes = line_text.as_bytes();
    let mut start = slot_local_start;
    while start < slot_local_end && matches!(bytes[start], b' ' | b'\t') {
        start += 1;
    }
    let mut end = slot_local_end;
    while end > start && matches!(bytes[end - 1], b' ' | b'\t') {
        end -= 1;
    }
    Some((row.line_start + start)..(row.line_start + end))
}
