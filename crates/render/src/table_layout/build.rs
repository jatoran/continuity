//! Pipe-table layout build — walks one [`EvaluatedTable`]'s rope
//! slice line by line and assembles the [`TableCellLayout`] / column
//! width / column alignment / alignment-row metadata that
//! [`crate::table_paint`] consumes.
//!
//! Split out from `table_layout.rs` to keep that file under the 600-line
//! cap; the public dispatcher `compute_table_layouts` lives there and
//! delegates to [`build_one_table_layout`] here.
//!
//! Thread ownership: pure data; callable from any thread that holds the
//! per-frame snapshot inputs.

use continuity_decorate::{column_alignments, EvaluatedTable};
use ropey::Rope;

use super::cell_inline::compute_cell_inline;
use super::cell_wrap::{
    split_cell_on_br, wrap_cell_lines, wrap_raw_preserving, CellLine, CellSegment,
};
use super::directive::{parse_table_directive, TableDirective};
use super::parse_row::{
    build_col_alignments, compute_col_widths_dip, enumerate_lines, is_delimiter_line,
    parse_row_cells, resolve_cell_display,
};
use super::{
    TableCellLayout, TableLayout, MAX_TABLE_COL_WIDTH_DIP, MIN_TABLE_COL_WIDTH_DIP,
    TABLE_CELL_PAD_DIP,
};

pub(super) fn build_one_table_layout(
    table: &EvaluatedTable,
    rope: &Rope,
    caret_bytes: &[usize],
    col_width_overrides: &[super::TableColWidthOverride],
    measure: &mut dyn FnMut(&str) -> f32,
) -> Option<TableLayout> {
    let block_len = rope.len_bytes();
    if table.block_range.start >= block_len || table.block_range.end > block_len {
        return None;
    }
    // Decorations occasionally lag the rope by one or more revisions
    // (the worker re-parse hasn't caught up to a recent edit, or the
    // user just focus-switched to a buffer whose stored spectator
    // decorations were transformed forward across rope deltas). When
    // the lag puts a table's byte range across a multi-byte char,
    // `byte_slice` panics; previously seen at
    // `perf-snapshots/manual-lag_after-coalesce_20260518-003817.tsv`
    // ("Byte range does not align with char boundaries: range
    // 53451..54424"). Treat misalignment as "table can't be drawn
    // this frame" — the renderer paints the source bytes raw and the
    // next decoration delivery re-aligns the range.
    if rope.try_byte_to_char(table.block_range.start).is_err()
        || rope.try_byte_to_char(table.block_range.end).is_err()
    {
        return None;
    }
    // Extend the slice's end to the end of the current source line
    // containing `block_range.end` ONLY when `block_range.end` falls
    // mid-line. Decorations track byte ranges forward across rope
    // deltas but the transform can lag a single keystroke when the
    // user is typing fast inside a cell — the last typed character
    // then sits PAST `block_range.end` in the current rope. Without
    // the extension the chrome builder misses that char while the
    // body-text painter (which always reads the full current line)
    // does paint it, landing it just past the table's right border —
    // the "bleed out the side" visual bug. When `block_range.end`
    // already sits at a line boundary (the usual decorated state —
    // the table ends with a `\n` whose successor byte is the start
    // of the next line) the slice is already complete and pulling
    // in another line would absorb non-table content (e.g. a
    // heading immediately below the table).
    let extended_end = {
        let end_line = rope.byte_to_line(table.block_range.end);
        let end_line_start = rope.line_to_byte(end_line);
        if table.block_range.end == end_line_start {
            table.block_range.end
        } else if end_line + 1 < rope.len_lines() {
            rope.line_to_byte(end_line + 1)
        } else {
            block_len
        }
    };
    if rope.try_byte_to_char(extended_end).is_err() {
        return None;
    }
    let block_src: String = rope
        .byte_slice(table.block_range.start..extended_end)
        .into();
    if block_src.is_empty() {
        return None;
    }
    // Phase F — presentation directive (`<!--continuity:width=…;wrap=…-->`)
    // on the line immediately above the table. Controls explicit column
    // widths and the wrap/clip mode; absent → auto widths, wrap on.
    let directive = parse_directive_above(rope, table.block_range.start);
    let wrap_enabled = directive.as_ref().map(|d| d.wrap).unwrap_or(true);
    let alignments_from_delim = column_alignments(&block_src);
    let lines = enumerate_lines(&block_src);
    let mut cells: Vec<TableCellLayout> = Vec::new();
    let mut measurement_texts: Vec<String> = Vec::new();
    // Phase F — per-cell `<br>`-split segments for wrappable plain
    // cells, parallel to `cells`. `None` for formula / alignment /
    // caret-in-cell / empty cells, which take other line-building paths.
    // Wrapping is deferred to a second pass once the (capped) column
    // widths are known.
    let mut cell_segments: Vec<Option<Vec<CellSegment>>> = Vec::new();
    // Phase F — parallel flags marking the cell the user is editing
    // (caret inside). Editing cells render their raw source — markers
    // and literal `<br>` visible — but still byte-preservingly wrap to
    // the column width in pass 2 so a long cell stays fully visible
    // while editing instead of clipping to one line.
    let mut cell_editing: Vec<bool> = Vec::new();
    let mut header_seen = false;
    let mut col_count: usize = 0;
    let mut first_source_line: Option<u32> = None;
    let mut last_source_line: u32 = 0;
    let mut alignment_row_source_line: Option<u32> = None;
    for line_info in &lines {
        // `enumerate_lines` emits a synthetic trailing entry whenever
        // `block_src` ends with `\n` — same shape as a `lines()` walk
        // that "completes" the final newline. Its `offset_in_block`
        // equals `block_src.len()` and its `line_doc_start` lands one
        // byte past the table's `block_range.end`, which is the FIRST
        // byte of whatever follows the table. Skipping the entry here
        // keeps `[first_source_line, last_source_line]` strictly
        // describing rows that contain table content — without the
        // skip, `last_source_line` jumps to the heading / paragraph
        // immediately after the table and `covers_source_line()`
        // reports the table as extending into downstream content.
        if line_info.offset_in_block >= block_src.len() && line_info.is_blank {
            continue;
        }
        let line_doc_start = table.block_range.start + line_info.offset_in_block;
        let source_line_idx = rope.byte_to_line(line_doc_start) as u32;
        if first_source_line.is_none() {
            first_source_line = Some(source_line_idx);
        }
        if line_info.is_blank {
            // Blank intermediate lines (rare for a tree-sitter-md
            // PipeTable, but possible if the extension absorbs a
            // weird intermediate) shouldn't move the lower bound
            // either — only non-blank content extends it.
            continue;
        }
        last_source_line = source_line_idx;
        let line_text = line_info.text;
        let is_alignment = is_delimiter_line(line_text);
        if is_alignment && alignment_row_source_line.is_none() {
            // Record the delimiter's source line up front. The painter
            // needs this even when `parse_row_cells` drops the row
            // below — otherwise the hide pass strips the bytes and no
            // chrome paints, leaving a blank gap. Tree-sitter-md
            // produces exactly one delimiter row per table; later
            // matches are ignored.
            alignment_row_source_line = Some(source_line_idx);
        }
        let parsed = parse_row_cells(line_text, line_doc_start);
        if parsed.is_empty() {
            continue;
        }
        col_count = col_count.max(parsed.len());
        let is_header_row = !header_seen && !is_alignment;
        if is_header_row {
            header_seen = true;
        }
        for (col_index, cell) in parsed.iter().enumerate() {
            // Inclusive-on-both-ends test so a caret immediately after
            // the last source byte (the natural position after typing
            // the trailing character of a formula) still counts as
            // "in this cell" and suppresses the eval override.
            let caret_in_cell = caret_bytes
                .iter()
                .any(|c| *c >= cell.doc_range.start && *c <= cell.doc_range.end);
            let (resolved_text, is_formula) = if is_alignment {
                (String::new(), false)
            } else {
                resolve_cell_display(table, cell.doc_range.start, cell.text, caret_in_cell)
            };
            // Inline-markdown styling (`**bold**`, `_italic_`, …).
            // Only when the cell is plain text (no formula override),
            // not the alignment row, and the user isn't editing the
            // raw bytes — caret-in-cell keeps `**` markers visible so
            // typing operates on what the user sees. Formula cells
            // already show their evaluated value, not source.
            //
            // `measurement_text` is what the column-width pass uses;
            // it ALWAYS reflects the stripped/visible form (computed
            // from `cell.text`, the raw source, regardless of caret
            // position). Without this, moving the caret into a
            // cell with markers would widen the column on entry and
            // narrow it on exit — chrome cache invalidates each time
            // and visible columns jiggle. Width stays stable across
            // caret transitions when we measure against the same
            // text either way.
            // The user is editing this cell when a caret sits inside its
            // source and the cell carries plain text (formula / alignment
            // / empty cells take their own paths). Editing cells keep the
            // raw source visible but still wrap — handled in pass 2 via
            // `wrap_raw_preserving`.
            let is_editing_cell =
                caret_in_cell && !is_formula && !is_alignment && !resolved_text.is_empty();
            let (display_text, inline_runs, measurement_text, segments_opt) =
                if is_formula || is_alignment || resolved_text.is_empty() {
                    (resolved_text.clone(), Vec::new(), resolved_text, None)
                } else if caret_in_cell {
                    // Editing the raw bytes — show source verbatim
                    // (markers and literal `<br>` visible) so typing
                    // operates on what the user sees. The raw text is
                    // wrapped byte-preservingly in pass 2 (see
                    // `is_editing_cell` below) so a long cell stays fully
                    // visible while editing rather than clipping to one
                    // line; `display_text` keeps the whole raw payload for
                    // the caret-bar mapping and the cache content hash.
                    let measure_text = compute_cell_inline(cell.text).display_text;
                    (resolved_text, Vec::new(), measure_text, None)
                } else {
                    // Phase F — plain cell: split on `<br>`, inline-parse
                    // each segment, defer wrapping to the width pass.
                    // The column sizes to the widest segment (so a
                    // `<br>` cell fits its longest line, capped); the
                    // joined form feeds the cache content hash.
                    let segments: Vec<CellSegment> = split_cell_on_br(&resolved_text)
                        .into_iter()
                        .map(|segment| {
                            let parsed = compute_cell_inline(&segment);
                            CellSegment {
                                display_text: parsed.display_text,
                                inline_runs: parsed.inline_runs,
                            }
                        })
                        .collect();
                    let measure_text = segments
                        .iter()
                        .map(|segment| segment.display_text.as_str())
                        .max_by_key(|text| text.chars().count())
                        .unwrap_or("")
                        .to_string();
                    let joined = segments
                        .iter()
                        .map(|segment| segment.display_text.as_str())
                        .collect::<Vec<_>>()
                        .join(" ");
                    (joined, Vec::new(), measure_text, Some(segments))
                };
            cells.push(TableCellLayout {
                source_line: source_line_idx,
                col: col_index as u32,
                source_range: cell.doc_range.clone(),
                display_text,
                is_header: is_header_row,
                is_alignment_row: is_alignment,
                is_formula,
                inline_runs,
                lines: Vec::new(),
            });
            measurement_texts.push(measurement_text);
            cell_segments.push(segments_opt);
            cell_editing.push(is_editing_cell);
        }
    }
    if cells.is_empty() || col_count == 0 {
        return None;
    }
    let mut col_widths_dip = compute_col_widths_dip(&cells, &measurement_texts, col_count, measure);
    // Phase F — an explicit directive width overrides the auto-size for
    // that column (clamped to a generous hard cap), so the column keeps
    // the user's chosen width and content wraps / clips to it.
    if let Some(directive) = directive.as_ref() {
        for (col, width) in directive.widths.iter().enumerate() {
            if let (Some(width), Some(slot)) = (width, col_widths_dip.get_mut(col)) {
                *slot = width.clamp(MIN_TABLE_COL_WIDTH_DIP, MAX_TABLE_COL_WIDTH_DIP);
            }
        }
    }
    // Phase F — transient live-resize drag override wins over the
    // directive and the auto-size, so the column previews at the
    // dragged width (and wrapping / row heights reflow with it) before
    // the new width is committed to the directive.
    for over in col_width_overrides {
        if over.block_start == table.block_range.start {
            if let Some(slot) = col_widths_dip.get_mut(over.col as usize) {
                *slot = over
                    .width
                    .clamp(MIN_TABLE_COL_WIDTH_DIP, MAX_TABLE_COL_WIDTH_DIP);
            }
        }
    }
    let col_alignments = build_col_alignments(&alignments_from_delim, col_count);
    let total_width_dip = col_widths_dip.iter().copied().sum();
    let first_line = first_source_line.unwrap_or(0);
    // Phase F pass 2 — column widths are now fixed (and capped), so
    // wrap each plain cell's `<br>`-segments to the cell's inner width
    // and record the resulting visual lines. Formula / alignment /
    // caret-in-cell cells (`cell_segments[i] == None`) stay one line.
    for (idx, cell) in cells.iter_mut().enumerate() {
        let inner_width = col_widths_dip
            .get(cell.col as usize)
            .copied()
            .unwrap_or(0.0)
            - 2.0 * TABLE_CELL_PAD_DIP;
        cell.lines = if cell_editing.get(idx).copied().unwrap_or(false) {
            // Caret-in-cell: wrap the raw source byte-preservingly so the
            // user sees their full content wrapped in real time and the
            // in-cell caret bar can map a source byte to its wrapped row.
            wrap_raw_preserving(
                &cell.display_text,
                inner_width.max(1.0),
                wrap_enabled,
                measure,
            )
        } else {
            match cell_segments.get_mut(idx).and_then(|slot| slot.take()) {
                Some(segments) => {
                    wrap_cell_lines(segments, inner_width.max(1.0), wrap_enabled, measure)
                }
                None => vec![CellLine {
                    text: cell.display_text.clone(),
                    inline_runs: cell.inline_runs.clone(),
                }],
            }
        };
        // Mirror a single-line cell's styling back onto `inline_runs` so
        // consumers that still read the field (the cache content hash,
        // the active-cell affordance) see it. Multi-line cells leave
        // `inline_runs` empty — their per-line runs live on `lines`.
        cell.inline_runs = if cell.lines.len() == 1 {
            cell.lines[0].inline_runs.clone()
        } else {
            Vec::new()
        };
    }
    // Per-row display-row count = max cell line count on that source
    // line (floored at 1), indexed by `source_line - first_source_line`.
    let row_span = (last_source_line.saturating_sub(first_line) + 1) as usize;
    let mut row_display_rows = vec![1u32; row_span];
    for cell in &cells {
        if cell.source_line < first_line {
            continue;
        }
        if let Some(slot) = row_display_rows.get_mut((cell.source_line - first_line) as usize) {
            *slot = (*slot).max(cell.line_count());
        }
    }
    Some(TableLayout {
        block_range: table.block_range.clone(),
        first_source_line: first_line,
        last_source_line,
        col_widths_dip,
        col_alignments,
        cells,
        alignment_row_source_line,
        total_width_dip,
        row_display_rows,
        wrap_cells: wrap_enabled,
    })
}

/// Phase F — parse the presentation directive on the line immediately
/// above the table whose first byte is `block_start`. Returns `None`
/// when there is no line above, the slice is mid-char (decoration lag),
/// or the line is not a `<!--continuity:…-->` directive.
fn parse_directive_above(rope: &Rope, block_start: usize) -> Option<TableDirective> {
    let first_line = rope.byte_to_line(block_start);
    if first_line == 0 {
        return None;
    }
    let above_start = rope.line_to_byte(first_line - 1);
    let above_end = rope.line_to_byte(first_line);
    if rope.try_byte_to_char(above_start).is_err() || rope.try_byte_to_char(above_end).is_err() {
        return None;
    }
    let line: String = rope.byte_slice(above_start..above_end).into();
    parse_table_directive(line.trim_end_matches(['\n', '\r']))
}
