//! Active-cell affordance painting for pipe tables — the 2-DIP active
//! outline, the in-cell caret bar, and the translucent cell-selected
//! fill. Split out of `table_paint.rs` to keep that file under the
//! 600-line cap. As a child module it reaches the parent's private
//! `cell_rect`, `should_skip_alignment_row`, and `ACTIVE_CELL_*`
//! constants directly via `super::`.

use continuity_decorate::TableAlignment;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{ID2D1DeviceContext, ID2D1SolidColorBrush};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout, DWRITE_HIT_TEST_METRICS,
};

use super::{
    cell_at, cell_rect, should_skip_alignment_row, TableLinePlacement, ACTIVE_CELL_CARET_WIDTH_DIP,
    ACTIVE_CELL_OUTLINE_STROKE_DIP, ACTIVE_CELL_SELECTED_FILL_ALPHA,
};
use crate::table_layout::{TableCellLayout, TableLayout, TABLE_CELL_PAD_DIP};

/// Paint a 2-DIP outline AND a cell-local caret bar over any cell on
/// `source_line` whose `source_range` contains one of `caret_bytes`.
/// Caller is responsible for the per-line `SetTransform` — coordinates
/// are layout-local just like [`paint_table_visual_line`]. Cheap
/// O(visible cells × carets) per line; no-op when `caret_bytes` is
/// empty.
///
/// The caret bar paints AFTER the cell chrome (this function runs in
/// the post-replay pass for the focused pane) so it sits visibly on
/// top of the cell's `body_bg` fill. Without this, the per-line text
/// pass's caret is occluded by the chrome's `body_bg` mask.
///
/// `column_advance_dip` is the width of one monospace glyph — same
/// scalar used by `compute_col_widths_dip` to size columns. The
/// intra-cell caret x is `cell_left + TABLE_CELL_PAD_DIP +
/// (chars_before_caret * column_advance_dip)`. For non-monospace
/// fonts this is an approximation; cells stay readable because their
/// content stays under the column width either way. Alignment
/// (`Left`/`Center`/`Right`) is honored.
///
/// The outline pass is intentionally separate from
/// [`paint_table_visual_line`] so the chrome cache can bake the cell
/// chrome once and replay it across frames while the caret-dependent
/// outline + caret paint fresh each frame on top.
/// Paint the outline + (caret bar OR selection fill) for any cell on
/// `source_line` overlapped by a selection. Two visual states:
///
/// - **Cell selected** — the selection spans the cell's `source_range`
///   exactly (ordered start = cell start, ordered end = cell end).
///   Renders as: 2-DIP outline + translucent fill in the same brush.
///   No caret bar; the user is in "Excel selected" mode where
///   Delete/Backspace clear content and typing replaces it.
/// - **Cell editing** — the selection's head lies inside the cell's
///   `source_range` but the selection does not span the whole cell.
///   Renders as: 2-DIP outline + thin caret bar at the head position.
///
/// `selection_ranges[i]` is the ordered `(start, end)` byte pair of
/// `selection[i]`; `head_bytes[i]` is that selection's head byte
/// (used to position the caret bar). The two slices must have the
/// same length.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_active_cell_outline_line(
    ctx: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    layouts: &[TableLayout],
    placement: TableLinePlacement,
    head_bytes: &[usize],
    selection_ranges: &[(usize, usize)],
    column_advance_dip: f32,
    outline_brush: &ID2D1SolidColorBrush,
    caret_brush: &ID2D1SolidColorBrush,
) {
    if layouts.is_empty() || head_bytes.is_empty() {
        return;
    }
    for layout in layouts {
        if !layout.covers_source_line(placement.source_line) {
            continue;
        }
        if should_skip_alignment_row(layout, placement.source_line) {
            // The alignment row is decorative chrome with no editable
            // content — never outline it as "active."
            continue;
        }
        let col_count = layout.col_widths_dip.len();
        for col_index in 0..col_count {
            let cell = match cell_at(layout, placement.source_line, col_index as u32) {
                Some(c) => c,
                None => continue,
            };
            // Find a selection whose ordered (start, end) intersects
            // the cell's `source_range`. Prefer a fully-covering
            // selection so the "Excel selected" state takes priority
            // over "caret in cell" when both happen to match.
            let mut covering_selection: Option<(usize, usize)> = None;
            let mut editing_caret: Option<usize> = None;
            for (i, (sel_start, sel_end)) in selection_ranges.iter().copied().enumerate() {
                let covers = sel_start == cell.source_range.start
                    && sel_end == cell.source_range.end
                    && sel_start != sel_end;
                if covers {
                    covering_selection = Some((sel_start, sel_end));
                    editing_caret = None;
                    break;
                }
                let head = head_bytes.get(i).copied().unwrap_or(sel_start);
                if head >= cell.source_range.start && head <= cell.source_range.end {
                    editing_caret = Some(head);
                }
            }
            if covering_selection.is_none() && editing_caret.is_none() {
                continue;
            }
            // The outline / selected-fill spans the cell's full
            // (possibly multi-line, while editing a wrapped cell) height
            // so it frames every wrapped row, not just the first.
            let row_height_dip =
                placement.row_display_rows.max(1) as f32 * placement.line_height_dip;
            let rect = cell_rect(
                layout,
                col_index as u32,
                row_height_dip,
                placement.x_origin_dip,
            );
            // Outline always paints when the cell is active.
            unsafe {
                ctx.DrawRectangle(&rect, outline_brush, ACTIVE_CELL_OUTLINE_STROKE_DIP, None);
            }
            if covering_selection.is_some() {
                // Cell-selected state: translucent fill, no caret bar.
                unsafe {
                    outline_brush.SetOpacity(ACTIVE_CELL_SELECTED_FILL_ALPHA);
                    ctx.FillRectangle(&rect, outline_brush);
                    outline_brush.SetOpacity(1.0);
                }
                continue;
            }
            let caret_byte = editing_caret.unwrap_or(cell.source_range.start);
            paint_caret_bar_in_cell(
                ctx,
                dwrite,
                format,
                cell,
                layout
                    .col_alignments
                    .get(col_index)
                    .copied()
                    .unwrap_or(TableAlignment::Left),
                &rect,
                placement.line_height_dip,
                caret_byte,
                column_advance_dip,
                caret_brush,
            );
        }
    }
}

/// Paint a thin vertical caret bar inside `cell_rect` at the position
/// corresponding to `caret_byte` within the cell's `display_text`.
/// Honors column alignment so the bar lands where the text actually
/// sits — the cell-text painter aligns leading/center/right too.
///
/// Uses `IDWriteTextLayout::HitTestTextPosition` to measure the exact
/// glyph x for the caret offset under the current font — the chrome
/// painter builds its cell text layout the same way, so caret and
/// glyphs align under proportional fonts too. The monospace
/// `column_advance_dip` is only used as a fallback when the layout
/// build fails (which shouldn't happen in practice).
#[allow(clippy::too_many_arguments)]
fn paint_caret_bar_in_cell(
    ctx: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    cell: &TableCellLayout,
    alignment: TableAlignment,
    cell_rect: &D2D_RECT_F,
    line_height_dip: f32,
    caret_byte: usize,
    column_advance_dip: f32,
    caret_brush: &ID2D1SolidColorBrush,
) {
    let inner_left = cell_rect.left + TABLE_CELL_PAD_DIP;
    let inner_right = (cell_rect.right - TABLE_CELL_PAD_DIP).max(inner_left);
    // Locate the wrapped row the caret sits on. An editing cell's
    // `lines` are byte-preserving (`wrap_raw_preserving`), so the
    // cumulative line byte lengths map a source-byte caret to its
    // (row, offset-within-row) without re-deriving the wrap.
    let offset_in_cell = caret_byte.saturating_sub(cell.source_range.start);
    let (line_index, line_text, offset_in_line) = locate_caret_line(cell, offset_in_cell);
    let leading_x = measure_intra_cell_x(
        dwrite,
        format,
        line_text,
        offset_in_line,
        column_advance_dip,
    );
    let total_x = measure_intra_cell_x(
        dwrite,
        format,
        line_text,
        line_text.len(),
        column_advance_dip,
    );
    let caret_x = match alignment {
        TableAlignment::Left => inner_left + leading_x,
        TableAlignment::Center => {
            let inner_w = (inner_right - inner_left).max(0.0);
            let text_start = inner_left + ((inner_w - total_x) / 2.0).max(0.0);
            text_start + leading_x
        }
        TableAlignment::Right => {
            let trailing = (total_x - leading_x).max(0.0);
            inner_right - trailing
        }
    };
    let caret_x = caret_x.clamp(inner_left, inner_right);
    let top = cell_rect.top + line_index as f32 * line_height_dip;
    let bar = D2D_RECT_F {
        left: caret_x,
        top,
        right: caret_x + ACTIVE_CELL_CARET_WIDTH_DIP,
        bottom: top + line_height_dip,
    };
    unsafe {
        ctx.FillRectangle(&bar, caret_brush);
    }
}

/// Map a byte offset into the cell's source to the wrapped line it
/// falls on, returning `(line_index, line_text, offset_within_line)`.
/// Relies on the editing cell's `lines` being byte-preserving (every
/// source byte lands on exactly one line) so the cumulative byte
/// lengths reconstruct the offset. A boundary offset prefers the start
/// of the next row (where the caret visually continues), except at the
/// final row which also owns the end-of-cell position.
fn locate_caret_line(cell: &TableCellLayout, offset_in_cell: usize) -> (usize, &str, usize) {
    let line_count = cell.lines.len();
    let mut consumed = 0usize;
    for (index, line) in cell.lines.iter().enumerate() {
        let len = line.text.len();
        let is_last = index + 1 == line_count;
        let within = if is_last {
            offset_in_cell <= consumed + len
        } else {
            offset_in_cell < consumed + len
        };
        if within {
            return (
                index,
                line.text.as_str(),
                offset_in_cell.saturating_sub(consumed),
            );
        }
        consumed += len;
    }
    let last = line_count.saturating_sub(1);
    let line = cell.lines.last().map(|l| l.text.as_str()).unwrap_or("");
    (last, line, line.len())
}

/// Measure the x-offset (in DIPs) of `byte_offset_in_text` within
/// `text` as if it were laid out by the cell-text painter. Builds a
/// throwaway `IDWriteTextLayout` and calls `HitTestTextPosition`;
/// returns the monospace-approximation fallback when the layout
/// build fails.
fn measure_intra_cell_x(
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    text: &str,
    byte_offset_in_text: usize,
    column_advance_dip: f32,
) -> f32 {
    let chars_before = text
        .get(..byte_offset_in_text.min(text.len()))
        .map(|s| s.chars().count())
        .unwrap_or(0);
    let fallback = chars_before as f32 * column_advance_dip;
    let wide: Vec<u16> = text.encode_utf16().collect();
    if wide.is_empty() {
        return 0.0;
    }
    let layout: IDWriteTextLayout =
        match unsafe { dwrite.CreateTextLayout(&wide, format, f32::INFINITY, f32::INFINITY) } {
            Ok(l) => l,
            Err(_) => return fallback,
        };
    // UTF-16 code-unit count of the leading substring is the
    // `textPosition` HitTestTextPosition expects.
    let utf16_offset = text
        .get(..byte_offset_in_text.min(text.len()))
        .map(|s| s.encode_utf16().count() as u32)
        .unwrap_or(0);
    let mut x = 0.0f32;
    let mut y = 0.0f32;
    let mut metrics = DWRITE_HIT_TEST_METRICS::default();
    let result =
        unsafe { layout.HitTestTextPosition(utf16_offset, false, &mut x, &mut y, &mut metrics) };
    if result.is_err() {
        return fallback;
    }
    x
}
