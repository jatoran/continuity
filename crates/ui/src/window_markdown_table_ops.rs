//! `Window` impls for pipe-table structural mutations driven by the
//! right-click context menu (insert/delete row, insert/delete column,
//! delete entire table).
//!
//! Each handler:
//! 1. Reads the latest editor snapshot + decoration snapshot.
//! 2. Finds the [`EvaluatedTable`] whose `block_range` contains the
//!    primary caret. If none, returns `UnsupportedContext` so the
//!    command palette / menu reports "not in a table."
//! 3. Re-parses the block source into a row × column grid using the
//!    same delimiter / cell tokenization the render-side layout
//!    builder uses (`continuity_decorate::tables::column_alignments`
//!    for delimiter detection; pipe scanning inline here to avoid a
//!    cross-crate dep on render's private parser).
//! 4. Builds one or more `EditOp`s and dispatches them in descending
//!    byte order via [`continuity_core::EditorHandle::apply_edit`].
//!    A single user-visible undo group per command.
//!
//! The reads are O(table source size) and run on the UI thread; tables
//! are small (typical < 32 rows, < 16 cols) so this is well below the
//! keypress budget.

use continuity_command::Error;
use continuity_decorate::EvaluatedTable;
use continuity_text::{EditOp, Position, Range};

use crate::window::Window;
use crate::window_helpers::invalidate_hwnd;
use crate::window_view_context::map_ui_to_command_error;

mod cell_selection;

/// One source-line worth of parsed pipe-table row geometry. All byte
/// offsets are document-absolute.
pub(crate) struct ParsedRow {
    /// Source-line index in the document rope.
    pub(crate) source_line: u32,
    /// First byte of the line (document-absolute).
    pub(crate) line_start: usize,
    /// First byte past the line's newline (or rope end). Used to
    /// delete the whole line including its terminator.
    pub(crate) line_end_with_newline: usize,
    /// `true` when the line matches the GFM delimiter pattern
    /// (`|---|:---:|---|`).
    pub(crate) is_delimiter: bool,
    /// Byte ranges (document-absolute) of every pipe character on the
    /// line, in source order. The first pipe is the row's leading
    /// `|`; the last is the trailing `|`. Empty when the row has no
    /// leading pipe (malformed — the table-ops refuse to operate).
    pub(crate) pipes: Vec<usize>,
}

impl ParsedRow {
    /// Column count for this row = number of cells between adjacent
    /// pipes. `pipes.len() - 1` when both leading and trailing pipes
    /// are present; zero when malformed.
    pub(crate) fn col_count(&self) -> usize {
        self.pipes.len().saturating_sub(1)
    }

    /// Document-absolute byte range of cell `col`'s "slot" — the
    /// region between (and excluding) the surrounding pipes. Used as
    /// the deletion target when removing a column.
    pub(crate) fn cell_slot(&self, col: usize) -> Option<std::ops::Range<usize>> {
        if col + 1 >= self.pipes.len() {
            return None;
        }
        Some((self.pipes[col] + 1)..self.pipes[col + 1])
    }
}

/// Locate the [`EvaluatedTable`] whose `block_range` contains the
/// primary caret byte. Returns `(table, caret_byte)` so the caller
/// doesn't have to re-derive the caret position from the snapshot.
pub(crate) fn focused_table<'a>(
    rope: &ropey::Rope,
    selections: &[continuity_text::Selection],
    tables: &'a [EvaluatedTable],
) -> Option<(&'a EvaluatedTable, usize)> {
    let primary = selections.first()?;
    let line = primary.head.line as usize;
    let line_start = if line < rope.len_lines() {
        rope.line_to_byte(line)
    } else {
        rope.len_bytes()
    };
    let caret_byte = line_start + primary.head.byte_in_line as usize;
    let table = tables
        .iter()
        .find(|t| caret_byte >= t.block_range.start && caret_byte < t.block_range.end)?;
    Some((table, caret_byte))
}

/// Phase F — read line `line` of `rope` as an owned `String` (with its
/// trailing newline). Empty for an out-of-range line.
fn line_text(rope: &ropey::Rope, line: u32) -> String {
    let l = line as usize;
    if l >= rope.len_lines() {
        return String::new();
    }
    let start = rope.line_to_byte(l);
    let end = if l + 1 < rope.len_lines() {
        rope.line_to_byte(l + 1)
    } else {
        rope.len_bytes()
    };
    rope.byte_slice(start..end).into()
}

/// Phase F — find a pipe-table's header (first row) line by scanning up
/// from `anchor_line` (a line inside the table) through the contiguous
/// `|`-prefixed rows. Rope-truth, so it does not depend on a possibly
/// stale decoration `block_range`.
fn table_header_line_above(rope: &ropey::Rope, anchor_line: u32) -> u32 {
    let mut line = anchor_line.min((rope.len_lines() as u32).saturating_sub(1));
    while line > 0 {
        if line_text(rope, line - 1).trim_start().starts_with('|') {
            line -= 1;
        } else {
            break;
        }
    }
    line
}

/// Parse one source line of a pipe-table block.
fn parse_table_row(rope: &ropey::Rope, line_idx: u32, len_lines: usize) -> Option<ParsedRow> {
    let line = line_idx as usize;
    if line >= len_lines {
        return None;
    }
    let line_start = rope.line_to_byte(line);
    let line_end_with_newline = if line + 1 < len_lines {
        rope.line_to_byte(line + 1)
    } else {
        rope.len_bytes()
    };
    let raw: String = rope
        .byte_slice(line_start..line_end_with_newline)
        .to_string();
    // Strip trailing newline for parsing.
    let text = raw.trim_end_matches('\n').trim_end_matches('\r');
    let bytes = text.as_bytes();
    let mut pipes = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() && bytes[i + 1] == b'|' {
            // Escaped pipe — not a column delimiter.
            i += 2;
            continue;
        }
        if bytes[i] == b'|' {
            pipes.push(line_start + i);
        }
        i += 1;
    }
    Some(ParsedRow {
        source_line: line_idx,
        line_start,
        line_end_with_newline,
        is_delimiter: is_delimiter_line(text),
        pipes,
    })
}

/// `true` when every non-whitespace byte is `:`, `-`, or `|`, and at
/// least one `-` appears. Same predicate the renderer uses.
fn is_delimiter_line(line: &str) -> bool {
    let mut saw_dash = false;
    let mut saw_content = false;
    for byte in line.bytes() {
        match byte {
            b'-' => {
                saw_dash = true;
                saw_content = true;
            }
            b':' | b'|' => saw_content = true,
            b' ' | b'\t' => {}
            _ => return false,
        }
    }
    saw_dash && saw_content
}

/// Walk every source line covered by `table.block_range` and produce
/// one [`ParsedRow`] per line.
pub(crate) fn parse_table_rows(rope: &ropey::Rope, table: &EvaluatedTable) -> Vec<ParsedRow> {
    let mut out = Vec::new();
    let len_lines = rope.len_lines();
    let first_line = rope.byte_to_line(table.block_range.start) as u32;
    let mut line = first_line;
    while (line as usize) < len_lines {
        let row = match parse_table_row(rope, line, len_lines) {
            Some(r) => r,
            None => break,
        };
        if row.line_start >= table.block_range.end {
            break;
        }
        out.push(row);
        line += 1;
    }
    out
}

/// Find the source line of the caret, returning the matching
/// `ParsedRow` index in `rows` or `None` when the caret falls between
/// rows (rare; can happen mid-edit).
pub(crate) fn row_index_for_caret(rows: &[ParsedRow], caret_byte: usize) -> Option<usize> {
    rows.iter()
        .position(|r| caret_byte >= r.line_start && caret_byte < r.line_end_with_newline)
}

/// Find the column index containing `caret_byte` within `row`. Returns
/// `None` when the caret sits on a pipe character or outside the
/// row's pipe range.
pub(crate) fn col_index_for_caret(row: &ParsedRow, caret_byte: usize) -> Option<usize> {
    let n = row.col_count();
    for col in 0..n {
        let slot = row.cell_slot(col)?;
        if caret_byte >= slot.start && caret_byte <= slot.end {
            return Some(col);
        }
    }
    None
}

/// Determine the column count to use when synthesizing a new blank
/// row: the maximum across all rows in the table.
pub(crate) fn table_col_count(rows: &[ParsedRow]) -> usize {
    rows.iter().map(|r| r.col_count()).max().unwrap_or(0)
}

/// Format an empty body row with `cols` columns, e.g. `|   |   |\n`.
/// Three spaces inside each cell so an unedited skeleton row reads
/// as a placeholder rather than as a delimiter row.
pub(crate) fn format_blank_row(cols: usize) -> String {
    let mut out = String::with_capacity(1 + cols * 5 + 1);
    out.push('|');
    for _ in 0..cols {
        out.push_str("   |");
    }
    out.push('\n');
    out
}

impl Window {
    pub(crate) fn markdown_table_insert_row_impl(&mut self, above: bool) -> Result<(), Error> {
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
        let (table, caret_byte) = focused_table(rope, snap.selections(), &dec.evaluated_tables)
            .ok_or(Error::UnsupportedContext("caret not in a table"))?;
        let rows = parse_table_rows(rope, table);
        let row_idx = row_index_for_caret(&rows, caret_byte)
            .ok_or(Error::UnsupportedContext("caret not on a table row"))?;
        let cols = table_col_count(&rows).max(1);
        let blank = format_blank_row(cols);
        let target_line = if above {
            rows[row_idx].source_line
        } else {
            rows[row_idx].source_line + 1
        };
        let insert_at = Position::new(target_line, 0);
        self.editor
            .apply_edit(self.buffer_id, EditOp::insert(insert_at, blank))
            .map_err(|e| map_ui_to_command_error(crate::Error::Core(e)))?;
        self.cancel_display_prewarm_for_buffer(self.buffer_id);
        invalidate_hwnd(self.hwnd);
        Ok(())
    }

    pub(crate) fn markdown_table_insert_column_impl(&mut self, before: bool) -> Result<(), Error> {
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
        let (table, caret_byte) = focused_table(rope, snap.selections(), &dec.evaluated_tables)
            .ok_or(Error::UnsupportedContext("caret not in a table"))?;
        let rows = parse_table_rows(rope, table);
        let row_idx = row_index_for_caret(&rows, caret_byte)
            .ok_or(Error::UnsupportedContext("caret not on a table row"))?;
        // Determine target column from the caret. When the caret
        // sits on a pipe (between cells), default to the column to
        // its right (insert-before) or left (insert-after).
        let target_col_in_caret_row = col_index_for_caret(&rows[row_idx], caret_byte).unwrap_or(0);
        // Apply edits in DESCENDING byte order so earlier offsets
        // remain valid as we splice. Walk rows from bottom to top.
        for row in rows.iter().rev() {
            let n = row.col_count();
            if n == 0 {
                continue;
            }
            let target_col = target_col_in_caret_row.min(n.saturating_sub(1));
            let insert_byte = if before {
                row.pipes[target_col]
            } else {
                row.pipes[target_col + 1]
            };
            let cell_payload = if row.is_delimiter { "---|" } else { "   |" };
            // Decompose absolute byte into Position relative to this row's line.
            let byte_in_line = insert_byte.saturating_sub(row.line_start) as u32;
            let pos = Position::new(row.source_line, byte_in_line);
            self.editor
                .apply_edit(
                    self.buffer_id,
                    EditOp::insert(pos, cell_payload.to_string()),
                )
                .map_err(|e| map_ui_to_command_error(crate::Error::Core(e)))?;
        }
        self.cancel_display_prewarm_for_buffer(self.buffer_id);
        invalidate_hwnd(self.hwnd);
        Ok(())
    }

    pub(crate) fn markdown_table_delete_row_impl(&mut self) -> Result<(), Error> {
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
        let (table, caret_byte) = focused_table(rope, snap.selections(), &dec.evaluated_tables)
            .ok_or(Error::UnsupportedContext("caret not in a table"))?;
        let rows = parse_table_rows(rope, table);
        let row_idx = row_index_for_caret(&rows, caret_byte)
            .ok_or(Error::UnsupportedContext("caret not on a table row"))?;
        // Refuse to delete the alignment row or the only header row —
        // either would leave a malformed table that tree-sitter-md
        // would stop classifying as a PipeTable, and the user almost
        // certainly didn't mean that.
        if rows[row_idx].is_delimiter {
            return Err(Error::UnsupportedContext("cannot delete the alignment row"));
        }
        let body_rows = rows.iter().filter(|r| !r.is_delimiter).count();
        if body_rows <= 1 {
            return Err(Error::UnsupportedContext("cannot delete the only row"));
        }
        let start = Position::new(rows[row_idx].source_line, 0);
        let line_len = rows[row_idx]
            .line_end_with_newline
            .saturating_sub(rows[row_idx].line_start);
        let end = Position::new(rows[row_idx].source_line, line_len as u32);
        self.editor
            .apply_edit(
                self.buffer_id,
                EditOp::replace(Range::new(start, end), String::new()),
            )
            .map_err(|e| map_ui_to_command_error(crate::Error::Core(e)))?;
        self.cancel_display_prewarm_for_buffer(self.buffer_id);
        invalidate_hwnd(self.hwnd);
        Ok(())
    }

    pub(crate) fn markdown_table_delete_column_impl(&mut self) -> Result<(), Error> {
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
        let (table, caret_byte) = focused_table(rope, snap.selections(), &dec.evaluated_tables)
            .ok_or(Error::UnsupportedContext("caret not in a table"))?;
        let rows = parse_table_rows(rope, table);
        let row_idx = row_index_for_caret(&rows, caret_byte)
            .ok_or(Error::UnsupportedContext("caret not on a table row"))?;
        let target_col = col_index_for_caret(&rows[row_idx], caret_byte)
            .ok_or(Error::UnsupportedContext("caret not in a cell"))?;
        let max_cols = table_col_count(&rows);
        if max_cols <= 1 {
            return Err(Error::UnsupportedContext("cannot delete the only column"));
        }
        // Delete from each row in descending byte order. The cell slot
        // is the bytes between its surrounding pipes. We delete the
        // trailing pipe AS WELL so the row's pipe count decreases by
        // one — except for the leftmost column, where we delete the
        // LEADING pipe instead so the row still starts with `|`.
        for row in rows.iter().rev() {
            let n = row.col_count();
            if n == 0 || target_col >= n {
                continue;
            }
            let (delete_start, delete_end) = if target_col == 0 {
                // Delete from leading pipe through (and including) the
                // cell content. Next pipe over becomes the new leading.
                (row.pipes[0], row.pipes[1])
            } else {
                // Delete from one pipe past the previous column through
                // (and including) this column's trailing pipe.
                (row.pipes[target_col], row.pipes[target_col + 1])
            };
            let start_in_line = delete_start.saturating_sub(row.line_start) as u32;
            let end_in_line = delete_end.saturating_sub(row.line_start) as u32;
            let start = Position::new(row.source_line, start_in_line);
            let end = Position::new(row.source_line, end_in_line);
            self.editor
                .apply_edit(
                    self.buffer_id,
                    EditOp::replace(Range::new(start, end), String::new()),
                )
                .map_err(|e| map_ui_to_command_error(crate::Error::Core(e)))?;
        }
        self.cancel_display_prewarm_for_buffer(self.buffer_id);
        invalidate_hwnd(self.hwnd);
        Ok(())
    }

    pub(crate) fn markdown_table_delete_table_impl(&mut self) -> Result<(), Error> {
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
        let (table, _caret_byte) = focused_table(rope, snap.selections(), &dec.evaluated_tables)
            .ok_or(Error::UnsupportedContext("caret not in a table"))?;
        let block_start = table.block_range.start;
        let block_end = table.block_range.end;
        let start_line = rope.byte_to_line(block_start);
        let start_in_line = (block_start - rope.line_to_byte(start_line)) as u32;
        let end_line = rope.byte_to_line(block_end);
        let end_in_line = (block_end - rope.line_to_byte(end_line)) as u32;
        let start = Position::new(start_line as u32, start_in_line);
        let end = Position::new(end_line as u32, end_in_line);
        self.editor
            .apply_edit(
                self.buffer_id,
                EditOp::replace(Range::new(start, end), String::new()),
            )
            .map_err(|e| map_ui_to_command_error(crate::Error::Core(e)))?;
        // Explicit cache invalidation. The per-paint
        // `build_focused_pane_table_layouts` falls back to the cached
        // prior layout whenever `compute_table_layouts` comes back
        // empty (the right call for the decorate-lag case under
        // typing — keeps the chrome continuous instead of flashing
        // on and off per keystroke). Delete-table is the one case
        // where empty is genuinely correct, so the handler that
        // performed the delete clears the cache here.
        self.last_focused_table_layouts
            .borrow_mut()
            .remove(&self.buffer_id);
        self.cancel_display_prewarm_for_buffer(self.buffer_id);
        invalidate_hwnd(self.hwnd);
        Ok(())
    }

    /// Phase F — toggle the focused table's wrap/clip mode by rewriting
    /// its `<!--continuity:…-->` directive on the line above the table
    /// (inserting one when absent). Existing column widths are preserved.
    pub(crate) fn markdown_table_toggle_wrap_impl(&mut self) -> Result<(), Error> {
        let anchor_line = {
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
            let (_table, caret_byte) =
                focused_table(rope, snap.selections(), &dec.evaluated_tables)
                    .ok_or(Error::UnsupportedContext("caret not in a table"))?;
            rope.byte_to_line(caret_byte) as u32
        };
        self.rewrite_table_directive_for_anchor(anchor_line, |directive| {
            directive.wrap = !directive.wrap;
        })
    }

    /// Phase F — commit a resized column's width to the table at
    /// `block_start` by rewriting its `<!--continuity:width=…-->`
    /// directive (inserting one when absent). Other columns' widths and
    /// the wrap mode are preserved. Called when a column-resize drag ends.
    pub(crate) fn commit_table_col_width(
        &mut self,
        block_start: usize,
        col: u32,
        width: f32,
    ) -> Result<(), Error> {
        let anchor_line = {
            let snap = self
                .editor
                .snapshot(self.buffer_id)
                .ok_or(Error::UnsupportedContext("no buffer"))?;
            let rope = snap.rope_snapshot().rope();
            let clamped = block_start.min(rope.len_bytes());
            rope.byte_to_line(clamped) as u32
        };
        self.rewrite_table_directive_for_anchor(anchor_line, move |directive| {
            if directive.widths.len() <= col as usize {
                directive.widths.resize(col as usize + 1, None);
            }
            directive.widths[col as usize] = Some(width.round());
        })
    }

    /// Phase F — rewrite (or insert) the `<!--continuity:…-->` directive
    /// for the table whose header is found by scanning the rope up from
    /// `anchor_line`. `mutate` adjusts the parsed directive in place
    /// (preserving the fields it doesn't touch). Any *duplicate*
    /// directive lines that accumulated above the table are collapsed
    /// into the single rewritten line, so a stale-decoration double-write
    /// self-heals on the next change. One undo group.
    fn rewrite_table_directive_for_anchor(
        &mut self,
        anchor_line: u32,
        mutate: impl FnOnce(&mut continuity_render::TableDirective),
    ) -> Result<(), Error> {
        let snap = self
            .editor
            .snapshot(self.buffer_id)
            .ok_or(Error::UnsupportedContext("no buffer"))?;
        let rope = snap.rope_snapshot().rope();
        let header_line = table_header_line_above(rope, anchor_line);
        // Walk up the contiguous directive lines directly above the
        // header; the one nearest the table seeds the edit and the whole
        // block is collapsed to one line.
        let mut dir_start = header_line;
        let mut nearest: Option<continuity_render::TableDirective> = None;
        while dir_start > 0 {
            let line = line_text(rope, dir_start - 1);
            if continuity_render::is_table_directive_line(&line) {
                if nearest.is_none() {
                    nearest = continuity_render::parse_table_directive(
                        line.trim_end_matches(['\n', '\r']),
                    );
                }
                dir_start -= 1;
            } else {
                break;
            }
        }
        let mut directive = nearest.unwrap_or_default();
        mutate(&mut directive);
        let new_line = continuity_render::format_table_directive(&directive.widths, directive.wrap);
        let op = if dir_start < header_line {
            EditOp::replace(
                Range::new(Position::new(dir_start, 0), Position::new(header_line, 0)),
                format!("{new_line}\n"),
            )
        } else {
            EditOp::insert(Position::new(header_line, 0), format!("{new_line}\n"))
        };
        self.editor
            .apply_edit(self.buffer_id, op)
            .map_err(|e| map_ui_to_command_error(crate::Error::Core(e)))?;
        self.last_focused_table_layouts
            .borrow_mut()
            .remove(&self.buffer_id);
        self.cancel_display_prewarm_for_buffer(self.buffer_id);
        invalidate_hwnd(self.hwnd);
        Ok(())
    }
}
