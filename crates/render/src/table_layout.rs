//! Pipe-table visual layout — pure per-frame metadata consumed by
//! [`crate::table_paint`].
//!
//! When the caret is outside a markdown pipe-table block, the renderer
//! draws each cell with its own borders, header background, and
//! per-column alignment (parsed from the `:---:` row). This module
//! produces the per-table layout: which source lines the table covers,
//! how wide each column should be in DIPs, what alignment each column
//! takes, and what text the painter should render in every cell
//! (formula source bytes get swapped for the evaluated value here, so
//! the per-cell painter doesn't re-run the F4 swap).
//!
//! Tables whose `block_range` contains any caret byte are excluded:
//! the user is editing the raw markdown, and the display-map hide pass
//! likewise leaves their pipes visible.
//!
//! Thread ownership: pure data; callable from the UI thread that owns
//! the renderer or, equivalently, from any worker that holds the
//! per-frame snapshot inputs.

use std::ops::Range;

use continuity_decorate::{EvaluatedTable, TableAlignment};
use continuity_display_map::{ImageRowReservation, SourceLine, SpanStyle};
use ropey::Rope;

mod build;
mod cell_inline;
pub(crate) mod cell_wrap;
pub(crate) mod directive;
mod parse_row;

use build::build_one_table_layout;
use cell_wrap::CellLine;

/// Floor for a single column's width in DIPs. Stops empty cells from
/// collapsing into invisible columns and gives a 1-column-with-spaces
/// fixture a sensible visual minimum.
pub const MIN_TABLE_COL_WIDTH_DIP: f32 = 64.0;

/// Phase F — auto-size ceiling for a column in DIPs. A column with no
/// explicit width directive sizes to its content up to this cap; wider
/// content wraps (or clips) instead of stretching the column. Modest so
/// long prose cells wrap rather than dragging the column across the
/// pane.
pub const DEFAULT_TABLE_COL_WIDTH_MAX_DIP: f32 = 220.0;

/// Phase F — hard ceiling for an *explicit* column width (set by a
/// directive / column drag). Larger than the auto cap so a user can
/// deliberately widen a column, but bounded so a malformed directive
/// can't blow the layout out.
pub const MAX_TABLE_COL_WIDTH_DIP: f32 = 800.0;

/// Horizontal padding inside each cell on both edges, in DIPs. The
/// content width is `text_width + 2 * TABLE_CELL_PAD_DIP`; the column
/// then takes the max of that and `MIN_TABLE_COL_WIDTH_DIP`.
pub const TABLE_CELL_PAD_DIP: f32 = 8.0;

/// One cell of a visual table.
#[derive(Clone, Debug, PartialEq)]
pub struct TableCellLayout {
    /// Document source line this cell sits on.
    pub source_line: u32,
    /// 0-indexed column position.
    pub col: u32,
    /// Document-absolute byte range of the trimmed cell payload (the
    /// content between the surrounding pipes, leading/trailing
    /// whitespace stripped). Empty body cells carry an empty range
    /// where `start == end`. Used to ask "does a caret head fall
    /// inside this cell?" for the active-cell visual outline, and by
    /// cell-aware navigation/edit commands.
    pub source_range: Range<usize>,
    /// Display text for the painter. Formula cells carry the
    /// evaluated value (e.g. `"42"` for `=SUM(A1:A3)`); error sentinels
    /// remain (`"#DIV/0!"`, `"#ERR"`).
    pub display_text: String,
    /// `true` for the header row's cells.
    pub is_header: bool,
    /// `true` when the cell sits inside the table's `:---:` alignment
    /// row. The painter draws borders for these cells but leaves
    /// `display_text` empty so the visual table reads as
    /// "header / [thin separator with borders] / body".
    pub is_alignment_row: bool,
    /// `true` when `display_text` was produced by the F4 formula
    /// evaluator (the cell's source bytes started with `=`). The
    /// painter uses this to pick the `formula.value` / `formula.error`
    /// foreground brushes instead of the body foreground.
    pub is_formula: bool,
    /// Per-byte style runs indexing into `display_text` (UTF-8 byte
    /// ranges). Populated when the cell's source carries inline
    /// markdown markers (`**bold**`, `_italic_`, `` `code` ``,
    /// `~~strike~~`, `[link](url)`). Empty for caret-in-cell cells
    /// (display text is raw source so the user sees their markers),
    /// for formula cells (display text is the evaluated value), and
    /// for alignment-row cells (no text).
    pub inline_runs: Vec<(Range<u32>, SpanStyle)>,
    /// Phase F — the cell's content split into the visual lines the
    /// painter stacks: first on `<br>` hard breaks, then greedy
    /// word-wrap of each segment to the (capped) column inner width.
    /// Always carries at least one entry (a single-line cell is one
    /// `CellLine` whose `text` equals `display_text`). The painter draws
    /// each line with no further wrapping so the rendered line count
    /// exactly equals `lines.len()` — which is what the row reservation
    /// reserves. `inline_runs` ride on a line only when that line was an
    /// unwrapped whole segment; wrapped sub-lines render plain
    /// (multi-line inline styling is a follow-up).
    pub lines: Vec<CellLine>,
}

impl TableCellLayout {
    /// Number of visual display rows this cell occupies (`lines.len()`,
    /// floored at 1).
    #[must_use]
    pub fn line_count(&self) -> u32 {
        self.lines.len().max(1) as u32
    }
}

impl TableCellLayout {
    /// `true` when any caret byte falls within this cell's
    /// `source_range`. Inclusive of the start byte, inclusive of the
    /// end (so a caret immediately after the last content byte still
    /// counts as in-cell — matches the user's intuition that "the
    /// caret is in this cell" when they typed a character there).
    #[must_use]
    pub fn contains_caret(&self, caret_bytes: &[usize]) -> bool {
        caret_bytes
            .iter()
            .any(|c| *c >= self.source_range.start && *c <= self.source_range.end)
    }
}

/// One pipe-table block laid out for visual rendering. Coordinates are
/// table-local: the painter translates by the line's body origin and
/// the table's column accumulators (`cell_x_dip`).
#[derive(Clone, Debug)]
pub struct TableLayout {
    /// Document-absolute byte range covered by this table block.
    /// Painters use it as an identity key when gating overlapping
    /// effects (e.g. the F4 swap painter skips tables that have a
    /// `TableLayout`).
    pub block_range: Range<usize>,
    /// First (inclusive) source line covered by this table block.
    pub first_source_line: u32,
    /// Last (inclusive) source line covered by this table block.
    pub last_source_line: u32,
    /// Per-column DIP widths in source order. `col_widths_dip[i]`
    /// covers column `i`; `cell_x_dip(i)` is the left edge.
    pub col_widths_dip: Vec<f32>,
    /// Per-column alignment parsed from the `:---:` delimiter row.
    /// `Left` when the table has no delimiter row or fewer alignment
    /// cells than columns.
    pub col_alignments: Vec<TableAlignment>,
    /// All cells in the table, in source-line order then column order.
    pub cells: Vec<TableCellLayout>,
    /// Source-line index of the table's `:---:` delimiter row when one
    /// was detected during layout build. Populated even when
    /// `parse_row_cells` produced no entries for that line (malformed
    /// delimiter without a leading pipe, transient mid-edit state) so
    /// the painter can still draw the per-column chrome over a slot the
    /// display map will hide. `None` for tables with no delimiter row.
    pub alignment_row_source_line: Option<u32>,
    /// Sum of `col_widths_dip` — handy for the painter's outer-rect
    /// math.
    pub total_width_dip: f32,
    /// Phase F — display-row count for every source line the table
    /// covers, indexed by `source_line - first_source_line`. A row with
    /// a wrapped or `<br>`-split cell occupies >1 display row; every
    /// other row is `1`. Drives the per-row cell-rect height, the
    /// chrome recorder's cumulative vertical offset, and the display-map
    /// reservations so body / gutter / caret below a tall row stay
    /// aligned. Length is always `last_source_line - first_source_line + 1`.
    pub row_display_rows: Vec<u32>,
    /// Phase F — whether cells wrap (`true`) or clip (`false`), parsed
    /// from the table's `wrap=` directive (default `true`). The UI reads
    /// this to seed the right-click "toggle wrap" menu and to round-trip
    /// the current state when rewriting the directive.
    pub wrap_cells: bool,
}

impl TableLayout {
    /// Left edge of column `col` in DIPs relative to the table's
    /// origin.
    #[must_use]
    pub fn cell_x_dip(&self, col: u32) -> f32 {
        self.col_widths_dip.iter().take(col as usize).copied().sum()
    }

    /// Phase F — display-row count for `source_line` (1 for a normal
    /// row, >1 when a cell wraps or carries `<br>`). Returns 1 for a
    /// source line outside the table's range (defensive).
    #[must_use]
    pub fn row_height(&self, source_line: u32) -> u32 {
        if source_line < self.first_source_line {
            return 1;
        }
        let idx = (source_line - self.first_source_line) as usize;
        self.row_display_rows.get(idx).copied().unwrap_or(1).max(1)
    }

    /// Phase F — cumulative display-row offset of `source_line` from the
    /// table's first source line, summing the heights of every earlier
    /// row. The chrome recorder multiplies this by `line_height` to
    /// stack variable-height rows; it equals
    /// `frame_display.first_display_line_index_for_source(source_line) -
    /// first_display_row` when the reservations match the row heights.
    #[must_use]
    pub fn display_row_offset_within_table(&self, source_line: u32) -> u32 {
        let clamped = source_line.max(self.first_source_line);
        (self.first_source_line..clamped)
            .map(|row| self.row_height(row))
            .sum()
    }

    /// Phase F — total display rows the whole table block occupies.
    #[must_use]
    pub fn total_display_rows(&self) -> u32 {
        self.row_display_rows
            .iter()
            .copied()
            .map(|h| h.max(1))
            .sum()
    }

    /// `true` when any row in the table occupies more than one display
    /// row — i.e. the table produces reservations the display map must
    /// honour.
    #[must_use]
    pub fn has_multiline_rows(&self) -> bool {
        self.row_display_rows.iter().any(|&h| h > 1)
    }

    /// `true` when `source_line` falls within `[first_source_line, last_source_line]`.
    #[must_use]
    pub fn covers_source_line(&self, source_line: u32) -> bool {
        source_line >= self.first_source_line && source_line <= self.last_source_line
    }

    /// Iterator over every cell sitting on `source_line`.
    pub fn cells_for_source_line(
        &self,
        source_line: u32,
    ) -> impl Iterator<Item = &TableCellLayout> + '_ {
        self.cells
            .iter()
            .filter(move |c| c.source_line == source_line)
    }

    /// `true` when `source_line` is the table's alignment row.
    ///
    /// Consults `alignment_row_source_line` first so callers get the
    /// right answer even when `parse_row_cells` produced no entries
    /// for the delimiter row.
    #[must_use]
    pub fn is_alignment_row(&self, source_line: u32) -> bool {
        if self.alignment_row_source_line == Some(source_line) {
            return true;
        }
        self.cells
            .iter()
            .any(|c| c.source_line == source_line && c.is_alignment_row)
    }

    /// `true` when `source_line` is the header row.
    #[must_use]
    pub fn is_header_row(&self, source_line: u32) -> bool {
        self.cells
            .iter()
            .any(|c| c.source_line == source_line && c.is_header)
    }
}

/// Compute one [`TableLayout`] per pipe-table block.
///
/// `tables` is `Decorations::evaluated_tables`. `measure` returns the
/// rendered width of `text` in DIPs under the active text format —
/// the production path wires this to DirectWrite; tests typically
/// pass a monospace approximation.
///
/// `caret_bytes` is the set of document-absolute caret byte positions
/// (one per selection head). Used to suppress formula-evaluation
/// overrides for the cell the user is currently editing: a partial
/// formula like `=` would otherwise render as `#ERR` mid-keystroke,
/// hiding the actual source bytes the user is typing. Cells whose
/// source range contains no caret keep their override behaviour.
///
/// Tables are laid out unconditionally; caret position no longer
/// gates whether a table renders. The painter draws an active-cell
/// outline over whichever cell currently contains a caret head so
/// the user can see where edits will land.
#[must_use]
pub fn compute_table_layouts(
    tables: &[EvaluatedTable],
    rope: &Rope,
    caret_bytes: &[usize],
    suppressed_table_blocks: &[Range<usize>],
    measure: &mut dyn FnMut(&str) -> f32,
) -> Vec<TableLayout> {
    compute_table_layouts_with_overrides(
        tables,
        rope,
        caret_bytes,
        suppressed_table_blocks,
        &[],
        measure,
    )
}

/// Phase F — a transient per-column width override, applied on top of
/// the directive widths. Used to preview a live column-resize drag
/// without first writing the new width to the rope: the UI paints the
/// column at `width` every frame while the user drags, then commits the
/// final width to the table's `<!--continuity:width=…-->` directive on
/// release. Because the override flows through the normal layout build,
/// the live drag reflows wrapping and row reservations correctly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TableColWidthOverride {
    /// Identifies the table by its source `block_range.start`.
    pub block_start: usize,
    /// 0-indexed column being resized.
    pub col: u32,
    /// Live width in DIPs.
    pub width: f32,
}

/// [`compute_table_layouts`] with transient per-column width overrides
/// (a live resize drag). Overrides win over both the auto-size and the
/// directive width for the matching `(block_start, col)`.
#[must_use]
pub fn compute_table_layouts_with_overrides(
    tables: &[EvaluatedTable],
    rope: &Rope,
    caret_bytes: &[usize],
    suppressed_table_blocks: &[Range<usize>],
    col_width_overrides: &[TableColWidthOverride],
    measure: &mut dyn FnMut(&str) -> f32,
) -> Vec<TableLayout> {
    let mut out = Vec::new();
    for table in tables {
        if suppressed_table_blocks
            .iter()
            .any(|r| r.start == table.block_range.start && r.end == table.block_range.end)
        {
            // Selection has reached past a single cell of this table —
            // skip the visual layout so the renderer paints raw
            // markdown (pipes, alignment row, formula source) and the
            // user can see what's actually selected. The F4 swap
            // painter still renders formula values inline at the
            // source-byte position (it's gated on the absence of a
            // `TableLayout`).
            continue;
        }
        if let Some(layout) =
            build_one_table_layout(table, rope, caret_bytes, col_width_overrides, measure)
        {
            out.push(layout);
        }
    }
    out
}

/// Phase F — derive the display-row reservations a set of table
/// layouts requires. Emits one [`ImageRowReservation`] for every table
/// source line that occupies more than one display row (a `<br>` /
/// wrapped row); single-row lines emit nothing. The result is the table
/// half of the merged reservation set the display map honours so body /
/// gutter / caret below a tall row stay aligned — the same mechanism
/// expanded inline images already use.
///
/// Output is **not** sorted or deduped here; the caller merges it with
/// the image reservations (taking the max per source line) and sorts
/// ascending before handing the set to the display-map walker.
#[must_use]
pub fn table_row_reservations(layouts: &[TableLayout]) -> Vec<ImageRowReservation> {
    let mut out = Vec::new();
    for layout in layouts {
        for (offset, &rows) in layout.row_display_rows.iter().enumerate() {
            if rows > 1 {
                out.push(ImageRowReservation {
                    source_line: SourceLine(layout.first_source_line + offset as u32),
                    reserved_display_rows: rows,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
