//! `Window` impls for cell-aware pipe-table navigation: Tab/Shift+Tab
//! to walk cells, Enter to drop into the cell below, Ctrl+Enter to
//! insert a literal `<br>`, and Up/Down to cross cell boundaries.
//!
//! Sibling of [`crate::window_markdown_table_ops`]; structural row /
//! column insert / delete commands live there. Navigation commands
//! split out so the structural-ops file stays under the 600-line cap.
//!
//! Each handler reads the same `(snapshot, rope, decorations,
//! caret_byte, parsed_rows)` quintet the structural commands use,
//! then either:
//!  - sets a new selection (motion only), or
//!  - dispatches a single `EditOp` and then sets the post-edit
//!    selection so the caret lands in the freshly-inserted location.
//!
//! All handlers return `UnsupportedContext` when the caret isn't in a
//! table cell so the keymap layer falls through to the global binding
//! (Tab → indent, Enter → insert newline, Up/Down → caret motion).

use continuity_command::Error;
use continuity_decorate::EvaluatedTable;
use continuity_text::{EditOp, Position, Selection, SelectionKind};

use crate::window::Window;
use crate::window_helpers::invalidate_hwnd;
use crate::window_markdown_table_ops::{
    col_index_for_caret, focused_table, format_blank_row, parse_table_rows, row_index_for_caret,
    table_col_count, ParsedRow,
};
use crate::window_view_context::map_ui_to_command_error;

/// Document-absolute trimmed-content byte range of the `col`-th cell
/// on `row`. Returns `None` when the row has fewer columns than `col`
/// or is the delimiter row.
fn cell_content_range(
    rope: &ropey::Rope,
    row: &ParsedRow,
    col: usize,
) -> Option<std::ops::Range<usize>> {
    if row.is_delimiter {
        return None;
    }
    let slot = row.cell_slot(col)?;
    let line_text: String = rope
        .byte_slice(row.line_start..row.line_end_with_newline)
        .to_string();
    let bytes = line_text.as_bytes();
    let slot_local_start = slot.start - row.line_start;
    let slot_local_end = slot.end - row.line_start;
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

/// Decompose `byte` into a `Position` (line + byte-in-line).
fn absolute_byte_to_position(rope: &ropey::Rope, byte: usize) -> Position {
    let clamped = byte.min(rope.len_bytes());
    let line = rope.byte_to_line(clamped);
    let line_start = rope.line_to_byte(line);
    Position::new(line as u32, (clamped - line_start) as u32)
}

/// Look up the primary caret's `(table, rows, row_idx, col_idx)`. All
/// four pieces are needed by every nav handler. Returns
/// `UnsupportedContext("...")` with a specific reason on miss so the
/// keymap layer can fall through.
fn primary_cell_position<'a>(
    rope: &ropey::Rope,
    selections: &[Selection],
    tables: &'a [EvaluatedTable],
) -> Result<(&'a EvaluatedTable, Vec<ParsedRow>, usize, usize, usize), Error> {
    let (table, caret_byte) = focused_table(rope, selections, tables)
        .ok_or(Error::UnsupportedContext("caret not in a table"))?;
    let rows = parse_table_rows(rope, table);
    let row_idx = row_index_for_caret(&rows, caret_byte)
        .ok_or(Error::UnsupportedContext("caret not on a table row"))?;
    let col_idx = col_index_for_caret(&rows[row_idx], caret_byte)
        .ok_or(Error::UnsupportedContext("caret not in a cell"))?;
    Ok((table, rows, row_idx, col_idx, caret_byte))
}

/// Find the source-line index of the first non-delimiter row at or
/// after `from_row` (walking direction = +1 for forward, -1 for
/// backward). Returns `None` when no non-delimiter row exists in that
/// direction inside the table.
fn nearest_body_row(rows: &[ParsedRow], from_row: usize, forward: bool) -> Option<usize> {
    if forward {
        rows.iter()
            .enumerate()
            .skip(from_row + 1)
            .find(|(_, r)| !r.is_delimiter)
            .map(|(i, _)| i)
    } else if from_row > 0 {
        rows[..from_row]
            .iter()
            .enumerate()
            .rev()
            .find(|(_, r)| !r.is_delimiter)
            .map(|(i, _)| i)
    } else {
        None
    }
}

impl Window {
    /// Tab (`forward = true`) / Shift+Tab (`forward = false`) cell walk.
    ///
    /// Forward: jump to the cell at the next column; at row-end, wrap
    /// to the next non-delimiter row's first cell; at the last cell of
    /// the last body row, insert a blank row and land in its first
    /// cell. Backward: jump to the previous column; wrap to previous
    /// row's last cell; at the first cell of the first body row, no-op.
    pub(crate) fn markdown_table_tab_step_impl(&mut self, forward: bool) -> Result<(), Error> {
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
        let (_table, rows, row_idx, col_idx, _caret_byte) =
            primary_cell_position(rope, snap.selections(), &dec.evaluated_tables)?;
        let col_count = rows[row_idx].col_count();
        if col_count == 0 {
            return Err(Error::UnsupportedContext("row has zero columns"));
        }
        // Compute the target (row_idx, col_idx). For forward, walking
        // off the end means: try next non-delimiter row's col 0. If
        // there's no next row, fall into the "extend" branch.
        let target = if forward {
            if col_idx + 1 < col_count {
                Some((row_idx, col_idx + 1))
            } else {
                nearest_body_row(&rows, row_idx, true).map(|next_row| (next_row, 0))
            }
        } else if col_idx > 0 {
            Some((row_idx, col_idx - 1))
        } else if let Some(prev_row) = nearest_body_row(&rows, row_idx, false) {
            let prev_cols = rows[prev_row].col_count();
            Some((prev_row, prev_cols.saturating_sub(1)))
        } else {
            // Shift+Tab at very first cell: no-op (the keymap layer
            // will treat this as handled — no global Shift+Tab does
            // anything especially useful in writing-mode anyway).
            return Ok(());
        };
        if let Some((target_row, target_col)) = target {
            if let Some(range) = cell_content_range(rope, &rows[target_row], target_col) {
                let pos = absolute_byte_to_position(rope, range.start);
                self.editor
                    .set_selections(
                        self.buffer_id,
                        vec![Selection::new(pos, pos, SelectionKind::Caret)],
                    )
                    .map_err(|e| map_ui_to_command_error(crate::Error::Core(e)))?;
                invalidate_hwnd(self.hwnd);
                return Ok(());
            }
        }
        // No existing target: extend by inserting a blank body row
        // after the caret's row, and place the caret at its first
        // cell. Auto-extend is forward-only — Shift+Tab returned above.
        if !forward {
            return Ok(());
        }
        self.insert_row_and_focus_cell(&rows, row_idx, 0)
    }

    /// Enter: jump to the cell directly below in the same column,
    /// skipping the delimiter row. At the last body row, insert a new
    /// blank row and land the caret at the same column.
    ///
    /// Returns `UnsupportedContext` when the caret sits at
    /// `byte_in_line == 0` of the table's first source line — the
    /// keymap chord-chain then falls through to the global Enter
    /// (`editor.insert_newline`) which inserts a `\n` at that
    /// position, pushing the table down by one line. Without the
    /// fall-through `editor.in_table` would still fire this handler
    /// and the row-step / row-extend branches would run instead,
    /// fighting the user's "shift the table down" intent.
    pub(crate) fn markdown_table_enter_impl(&mut self) -> Result<(), Error> {
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
        if let Some(primary) = snap.selections().first() {
            for table in &dec.evaluated_tables {
                let first_line = rope.byte_to_line(table.block_range.start) as u32;
                if primary.head.line == first_line && primary.head.byte_in_line == 0 {
                    return Err(Error::UnsupportedContext("caret at table start"));
                }
            }
        }
        let (_table, rows, row_idx, col_idx, _caret_byte) =
            primary_cell_position(rope, snap.selections(), &dec.evaluated_tables)?;
        if let Some(next_row) = nearest_body_row(&rows, row_idx, true) {
            let target_col = col_idx.min(rows[next_row].col_count().saturating_sub(1));
            if let Some(range) = cell_content_range(rope, &rows[next_row], target_col) {
                let pos = absolute_byte_to_position(rope, range.start);
                self.editor
                    .set_selections(
                        self.buffer_id,
                        vec![Selection::new(pos, pos, SelectionKind::Caret)],
                    )
                    .map_err(|e| map_ui_to_command_error(crate::Error::Core(e)))?;
                invalidate_hwnd(self.hwnd);
                return Ok(());
            }
        }
        // No body row below — extend the table.
        self.insert_row_and_focus_cell(&rows, row_idx, col_idx)
    }

    /// Ctrl+Enter inside a cell: insert literal `<br>` at the caret
    /// position so the cell content continues on a new visual line
    /// (Phase F renders the `<br>` as a real in-cell wrap).
    ///
    /// Resolves the caret through [`focused_table`] (caret-anywhere-in-
    /// table) rather than [`primary_cell_position`] (caret-pinned-to-a-
    /// cell): inserting `<br>` only needs the caret byte, so this fires
    /// even when the caret sits on a pipe, at a row edge, or in an empty
    /// cell. Routing through `primary_cell_position` returned
    /// `UnsupportedContext` in those spots and let the chord fall
    /// through to the global Ctrl+Enter, whose raw newline split the
    /// table.
    pub(crate) fn markdown_table_insert_break_impl(&mut self) -> Result<(), Error> {
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
        let (_table, caret_byte) = focused_table(rope, snap.selections(), &dec.evaluated_tables)
            .ok_or(Error::UnsupportedContext("caret not in a table"))?;
        let pos = absolute_byte_to_position(rope, caret_byte);
        self.editor
            .apply_edit(self.buffer_id, EditOp::insert(pos, "<br>".to_string()))
            .map_err(|e| map_ui_to_command_error(crate::Error::Core(e)))?;
        self.cancel_display_prewarm_for_buffer(self.buffer_id);
        invalidate_hwnd(self.hwnd);
        Ok(())
    }

    /// Shift+Enter inside a table: move the caret to the cell directly
    /// above (same column, skipping the alignment row) — the inverse of
    /// [`Self::markdown_table_enter_impl`]. Unlike
    /// [`Self::markdown_table_move_vertical_impl`] it never leaves the
    /// table: at the header row (no non-delimiter row above) it stays
    /// put, and any spot where the precise cell can't be resolved is a
    /// no-op too. Every miss returns `Ok` rather than
    /// `UnsupportedContext`, so the chord never falls through to the
    /// global Shift+Enter, whose raw newline would split the table.
    pub(crate) fn markdown_table_cell_up_impl(&mut self) -> Result<(), Error> {
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
        // Outside a table, fall through to the global Shift+Enter. The
        // keymap already gates this command on `editor.in_table`; this
        // guard keeps the two in agreement defensively.
        if focused_table(rope, snap.selections(), &dec.evaluated_tables).is_none() {
            return Err(Error::UnsupportedContext("caret not in a table"));
        }
        // In a table: keep the caret inside it no matter what. Every
        // unresolved case below stays put instead of breaking the table.
        let Ok((_table, rows, row_idx, col_idx, _caret_byte)) =
            primary_cell_position(rope, snap.selections(), &dec.evaluated_tables)
        else {
            return Ok(());
        };
        let Some(target_row) = nearest_body_row(&rows, row_idx, false) else {
            return Ok(());
        };
        let target_col = col_idx.min(rows[target_row].col_count().saturating_sub(1));
        let Some(range) = cell_content_range(rope, &rows[target_row], target_col) else {
            return Ok(());
        };
        let pos = absolute_byte_to_position(rope, range.start);
        self.editor
            .set_selections(
                self.buffer_id,
                vec![Selection::new(pos, pos, SelectionKind::Caret)],
            )
            .map_err(|e| map_ui_to_command_error(crate::Error::Core(e)))?;
        invalidate_hwnd(self.hwnd);
        Ok(())
    }

    /// Up (`down = false`) / Down (`down = true`) cell-row motion.
    /// Falls through with `UnsupportedContext` when the caret is at
    /// the top/bottom of the table (no neighbor exists), so the global
    /// Up/Down binding moves the caret out of the table normally.
    pub(crate) fn markdown_table_move_vertical_impl(&mut self, down: bool) -> Result<(), Error> {
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
        let (_table, rows, row_idx, col_idx, _caret_byte) =
            primary_cell_position(rope, snap.selections(), &dec.evaluated_tables)?;
        let target_row = nearest_body_row(&rows, row_idx, down)
            .ok_or(Error::UnsupportedContext("no neighbor row inside table"))?;
        let target_col = col_idx.min(rows[target_row].col_count().saturating_sub(1));
        let range = cell_content_range(rope, &rows[target_row], target_col)
            .ok_or(Error::UnsupportedContext("target cell missing"))?;
        let pos = absolute_byte_to_position(rope, range.start);
        self.editor
            .set_selections(
                self.buffer_id,
                vec![Selection::new(pos, pos, SelectionKind::Caret)],
            )
            .map_err(|e| map_ui_to_command_error(crate::Error::Core(e)))?;
        invalidate_hwnd(self.hwnd);
        Ok(())
    }

    /// Insert a blank body row immediately AFTER `caret_row_idx` and
    /// land the caret at `target_col` of the new row. Shared by Tab
    /// (target col = 0) and Enter (target col = same column).
    fn insert_row_and_focus_cell(
        &mut self,
        rows: &[ParsedRow],
        caret_row_idx: usize,
        target_col: usize,
    ) -> Result<(), Error> {
        let cols = table_col_count(rows).max(1);
        let blank = format_blank_row(cols);
        let new_source_line = rows[caret_row_idx].source_line + 1;
        let insert_at = Position::new(new_source_line, 0);
        self.editor
            .apply_edit(self.buffer_id, EditOp::insert(insert_at, blank))
            .map_err(|e| map_ui_to_command_error(crate::Error::Core(e)))?;
        // Now compute the caret position in the post-edit rope. The
        // newly-inserted blank row's first cell sits at
        // `byte_in_line = 1 + target_col * 4 + 1` for a `|   |   |…\n`
        // shape (each cell is 4 bytes: `   |`). Use parse_table_rows
        // logic against the new snapshot for robustness.
        let snap_after = self
            .editor
            .snapshot(self.buffer_id)
            .ok_or(Error::UnsupportedContext("buffer vanished mid-edit"))?;
        let rope_after = snap_after.rope_snapshot().rope();
        let id = self.buffer_id.as_uuid().as_u128();
        let dec = self
            .decoration_cache
            .get(id)
            .ok_or(Error::UnsupportedContext("no decorations after edit"))?;
        // Decorations may lag the rope by one revision (the parse
        // worker hasn't caught the insert yet). When that happens the
        // table-finder either misses the new row or still sees the
        // old shape. Fall back to a synthetic Position based on the
        // canonical `format_blank_row` shape so the caret lands
        // approximately correctly; the next decoration delivery will
        // re-align the active-cell outline.
        let caret_pos = if let Some(new_row) = parse_table_rows(
            rope_after,
            dec.evaluated_tables
                .iter()
                .find(|t| {
                    let bs = t.block_range.start;
                    let line0 = rope_after.byte_to_line(bs);
                    line0 as u32 <= new_source_line
                        && (rope_after.byte_to_line(t.block_range.end.saturating_sub(1)) as u32)
                            >= new_source_line
                })
                .unwrap_or(&EvaluatedTable {
                    block_range: 0..rope_after.len_bytes(),
                    overrides: Vec::new(),
                }),
        )
        .into_iter()
        .find(|r| r.source_line == new_source_line)
        .and_then(|r| cell_content_range(rope_after, &r, target_col))
        {
            absolute_byte_to_position(rope_after, new_row.start)
        } else {
            // Fallback: caret immediately after the leading `|` plus
            // `target_col * 4` bytes (each cell is `   |` = 4 bytes),
            // landing at the start of the target cell's whitespace.
            let byte_in_line = 1 + (target_col as u32) * 4 + 1;
            Position::new(new_source_line, byte_in_line)
        };
        self.editor
            .set_selections(
                self.buffer_id,
                vec![Selection::new(caret_pos, caret_pos, SelectionKind::Caret)],
            )
            .map_err(|e| map_ui_to_command_error(crate::Error::Core(e)))?;
        self.cancel_display_prewarm_for_buffer(self.buffer_id);
        invalidate_hwnd(self.hwnd);
        Ok(())
    }
}
