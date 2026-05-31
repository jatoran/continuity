//! Pipe-table visual rendering — D2D paint pass.
//!
//! Sibling of [`crate::table_formula_paint`]; the formula painter only
//! swaps source bytes for evaluated values one cell at a time, this
//! painter draws the full visual table: borders, header background,
//! per-column-aligned cell text, and a body-bg mask that erases the
//! flattened source glyphs (now that the display map has hidden the
//! pipes, only the trimmed cell content remains, but it sits at the
//! wrong x position for the visual layout).
//!
//! The painter runs **per source line** inside the existing per-line
//! transform (`SetTransform` to body-origin + line-y already applied by
//! [`crate::renderer_line_text_pass`] /
//! [`crate::wrap_paint::paint_display_lines`]). Coordinates are
//! layout-local: `(0, 0)` is the top-left of the current line's first
//! glyph.
//!
//! Thread ownership: UI thread (only owner of `ID2D1DeviceContext` and
//! the `IDWriteFactory`).

use continuity_decorate::TableAlignment;
use continuity_display_map::{SpanRole, SpanStyle};
use windows::Win32::Graphics::Direct2D::Common::{D2D_POINT_2F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_ALIASED,
    D2D1_DRAW_TEXT_OPTIONS_NONE,
};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout, DWRITE_FONT_STYLE_ITALIC,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING,
    DWRITE_TEXT_RANGE, DWRITE_WORD_WRAPPING_NO_WRAP,
};

use crate::table_layout::{TableCellLayout, TableLayout, TABLE_CELL_PAD_DIP};

mod active_cell;
pub(crate) use active_cell::paint_active_cell_outline_line;

/// Per-line placement scalars for the visual-table painter. Bundled so
/// the per-line entry point sits inside clippy's 7-argument cap without
/// an `#[allow]`.
pub(crate) struct TableLinePlacement {
    /// Document source-line index currently being painted.
    pub source_line: u32,
    /// Number of display rows the cell rect on this source line spans.
    /// The focused command-list path passes `layout.row_height(row)`
    /// (the cell-wrap line count). The spectator post-pass passes the
    /// frame's *actual* `display_line_count_for_source(row)` so the
    /// chrome tiles exactly over the projected rows even when the raw
    /// table line soft-wraps to more rows than the cell reserves —
    /// otherwise the unmasked wrap-continuation glyphs bleed through.
    pub row_display_rows: u32,
    /// Logical line height in DIPs.
    pub line_height_dip: f32,
    /// Table left edge in layout-local DIPs. Production paths pass
    /// `0.0` — the per-line `SetTransform` has already aligned the
    /// body origin to the cell grid.
    pub x_origin_dip: f32,
}

/// Brushes the visual painter consumes.
pub(crate) struct TableVisualBrushes<'a> {
    /// Body background — fills cell rect before drawing text so the
    /// underlying body glyphs don't bleed through. Same brush the
    /// renderer uses to clear the swap chain.
    pub body_bg: &'a ID2D1SolidColorBrush,
    /// Subtle fill behind every header-row cell.
    pub header_bg: &'a ID2D1SolidColorBrush,
    /// Fill behind the alignment-row slot (`markdown.table.alignment_bg`).
    /// Visually distinct from `header_bg` so the divider strip reads as
    /// its own band between the header and the body.
    pub alignment_bg: &'a ID2D1SolidColorBrush,
    /// Cell border — 1 DIP stroke around every cell.
    pub border: &'a ID2D1SolidColorBrush,
    /// Default cell-text foreground (plain cells).
    pub text_fg: &'a ID2D1SolidColorBrush,
    /// Formula-evaluator value foreground.
    pub formula_value: &'a ID2D1SolidColorBrush,
    /// Formula-evaluator error foreground.
    pub formula_error: &'a ID2D1SolidColorBrush,
}

/// Border stroke width in DIPs. 1 keeps the cell grid crisp without
/// dominating the visual.
const TABLE_BORDER_STROKE_DIP: f32 = 1.0;

/// Active-cell outline stroke width in DIPs. Slightly heavier than the
/// 1-DIP cell border so the "you are editing here" affordance reads
/// from a glance, without overpowering the chrome.
const ACTIVE_CELL_OUTLINE_STROKE_DIP: f32 = 2.0;

/// Active-cell caret bar width in DIPs. Same width as the standard
/// body-text caret so the in-cell caret matches the editor's
/// non-table caret style.
const ACTIVE_CELL_CARET_WIDTH_DIP: f32 = 1.0;

/// Alpha multiplier (0–1) applied to the caret brush when drawing the
/// "cell selected" fill — the same chroma as the outline so the two
/// affordances read together. 0.25 keeps the underlying cell text
/// legible while signalling the selected state at a glance.
const ACTIVE_CELL_SELECTED_FILL_ALPHA: f32 = 0.25;

/// Paint every visual-table cell that sits on `source_line` for any
/// table in `layouts`.
///
pub(crate) fn paint_table_visual_line(
    ctx: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    layouts: &[TableLayout],
    placement: TableLinePlacement,
    brushes: &TableVisualBrushes<'_>,
) {
    if layouts.is_empty() {
        return;
    }
    for layout in layouts {
        if !layout.covers_source_line(placement.source_line) {
            continue;
        }
        paint_layout_for_line(ctx, dwrite, format, layout, &placement, brushes);
    }
}

fn paint_layout_for_line(
    ctx: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    layout: &TableLayout,
    placement: &TableLinePlacement,
    brushes: &TableVisualBrushes<'_>,
) {
    if should_skip_alignment_row(layout, placement.source_line) {
        // Alignment row branch — display map has hidden the source
        // bytes; we paint a thin styled divider (body_bg fill +
        // per-column borders) so the slot reads as part of the
        // bordered table chrome. Driven by
        // `alignment_row_source_line` so it fires reliably regardless
        // of whether `parse_row_cells` produced cells for this row.
        // Line count stays 1:1 source ↔ display across caret-in /
        // caret-out modes.
        paint_alignment_row_dividers(ctx, layout, placement, brushes);
        return;
    }
    let any_cell_on_this_line = layout
        .cells
        .iter()
        .any(|c| c.source_line == placement.source_line);
    if !any_cell_on_this_line {
        return;
    }
    let col_count = layout.col_widths_dip.len();
    // Phase F — the row's display height in DIPs. The caller supplies
    // the display-row span: the focused path passes the cell-wrap line
    // count (`row_height`), the spectator path passes the frame's actual
    // projected row count so the cell fills mask every glyph the display
    // map laid out for this source line (a tall row reserves the same
    // rows so content below stays aligned).
    let row_height_dip = placement.row_display_rows.max(1) as f32 * placement.line_height_dip;
    // First pass: mask + header-fill, so subsequent borders + text sit
    // on top of the fills regardless of cell iteration order.
    for col_index in 0..col_count {
        let cell = match cell_at(layout, placement.source_line, col_index as u32) {
            Some(c) => c,
            None => continue,
        };
        let rect = cell_rect(
            layout,
            col_index as u32,
            row_height_dip,
            placement.x_origin_dip,
        );
        unsafe {
            ctx.FillRectangle(&rect, brushes.body_bg);
            if cell.is_header {
                ctx.FillRectangle(&rect, brushes.header_bg);
            }
        }
    }
    // Second pass: text + borders. Text sits inside the padded inner
    // rect; borders trace the full cell rect.
    for col_index in 0..col_count {
        let cell = match cell_at(layout, placement.source_line, col_index as u32) {
            Some(c) => c,
            None => continue,
        };
        let rect = cell_rect(
            layout,
            col_index as u32,
            row_height_dip,
            placement.x_origin_dip,
        );
        let alignment = layout
            .col_alignments
            .get(col_index)
            .copied()
            .unwrap_or(TableAlignment::Left);
        if !cell.display_text.is_empty() && !cell.is_alignment_row {
            draw_cell_text(
                ctx,
                dwrite,
                format,
                &rect,
                cell,
                alignment,
                placement.line_height_dip,
                brushes,
            );
        }
        draw_cell_border(ctx, &rect, brushes.border);
    }
    // Defensive overflow mask. The body-text painter (display-map
    // projection) lays out the full source line including any bytes
    // the chrome builder may have transiently missed — most often
    // when a single keystroke inside a cell lands a character past
    // the lagged `EvaluatedTable.block_range.end`. Without this
    // mask, that byte renders just past the table's right border
    // ("bleeds out the side"). Filling body_bg from `total_width_dip`
    // onward covers any such overflow inside the line's vertical
    // slot. `OVERFLOW_MASK_RIGHT_DIP` is wider than any reasonable
    // viewport; D2D clips to the bound surface so the over-large
    // rect is free.
    let overflow_rect = D2D_RECT_F {
        left: placement.x_origin_dip + layout.total_width_dip,
        top: 0.0,
        right: placement.x_origin_dip + layout.total_width_dip + OVERFLOW_MASK_RIGHT_DIP,
        bottom: row_height_dip,
    };
    unsafe {
        ctx.FillRectangle(&overflow_rect, brushes.body_bg);
    }
}

/// Width (DIPs) of the body_bg mask painted to the right of the
/// table's last cell. Sized larger than any reasonable display so a
/// stale single-keystroke overflow can't slip past on a wide window.
const OVERFLOW_MASK_RIGHT_DIP: f32 = 8192.0;

/// `true` when the visual painter should branch into the
/// alignment-row chrome path (column-divider verticals only) instead
/// of the standard cell-rect chrome. Pure predicate so the gate is
/// unit-testable without a D2D context.
#[must_use]
pub(crate) fn should_skip_alignment_row(layout: &TableLayout, source_line: u32) -> bool {
    layout.alignment_row_source_line == Some(source_line)
}

/// Paint the alignment row's slot as a styled separator strip:
/// `header_bg` fill spanning every column plus the standard 1-DIP cell
/// border around each. No horizontal rule and no text — the column
/// borders already connect the header chrome above to the body chrome
/// below, and the subtle fill distinguishes the slot from editor bg.
///
/// Driven by `layout.alignment_row_source_line` (not the per-cell
/// `is_alignment_row` flag) so the chrome always paints reliably even
/// when `parse_row_cells` produced no entries for the delimiter row
/// (malformed delimiter, transient mid-edit state). Cell `col_widths`
/// come from the header / body parse, so even a zero-cell alignment
/// row gets the right column grid.
fn paint_alignment_row_dividers(
    ctx: &ID2D1DeviceContext,
    layout: &TableLayout,
    placement: &TableLinePlacement,
    brushes: &TableVisualBrushes<'_>,
) {
    let col_count = layout.col_widths_dip.len();
    if col_count == 0 {
        return;
    }
    for col_index in 0..col_count {
        let left = placement.x_origin_dip + layout.cell_x_dip(col_index as u32);
        let width = layout.col_widths_dip.get(col_index).copied().unwrap_or(0.0);
        let rect = D2D_RECT_F {
            left,
            top: 0.0,
            right: left + width,
            bottom: placement.line_height_dip,
        };
        unsafe {
            ctx.FillRectangle(&rect, brushes.alignment_bg);
            ctx.DrawRectangle(&rect, brushes.border, TABLE_BORDER_STROKE_DIP, None);
        }
    }
}

fn cell_at(layout: &TableLayout, source_line: u32, col: u32) -> Option<&TableCellLayout> {
    layout
        .cells
        .iter()
        .find(|c| c.source_line == source_line && c.col == col)
}

fn cell_rect(layout: &TableLayout, col: u32, row_height_dip: f32, x_origin_dip: f32) -> D2D_RECT_F {
    let left = x_origin_dip + layout.cell_x_dip(col);
    let width = layout
        .col_widths_dip
        .get(col as usize)
        .copied()
        .unwrap_or(0.0);
    D2D_RECT_F {
        left,
        top: 0.0,
        right: left + width,
        bottom: row_height_dip,
    }
}

/// Draw a cell's visual lines stacked top-to-bottom. Each
/// [`crate::CellLine`] is laid out with **no** further
/// wrapping (the lines were already split / wrapped during layout
/// build), so the painted line count matches the reserved row height
/// exactly. Line `i` sits at `rect.top + i * line_height_dip`.
#[allow(clippy::too_many_arguments)]
fn draw_cell_text(
    ctx: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    rect: &D2D_RECT_F,
    cell: &TableCellLayout,
    alignment: TableAlignment,
    line_height_dip: f32,
    brushes: &TableVisualBrushes<'_>,
) {
    let inner_w = (rect.right - rect.left - 2.0 * TABLE_CELL_PAD_DIP).max(1.0);
    let line_h = line_height_dip.max(1.0);
    let dwrite_alignment = match alignment {
        TableAlignment::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
        TableAlignment::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
        TableAlignment::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
    };
    let brush = pick_text_brush(cell, brushes);
    // Clip every line to the cell rect so a clipped (`wrap=off`) cell —
    // or any slight measurement drift in a wrapped cell — never bleeds
    // into the neighbouring column. Push / pop are balanced (the only
    // early exit inside the loop is `continue`).
    unsafe {
        ctx.PushAxisAlignedClip(rect, D2D1_ANTIALIAS_MODE_ALIASED);
    }
    for (line_index, line) in cell.lines.iter().enumerate() {
        if line.text.is_empty() {
            continue;
        }
        let wide: Vec<u16> = line.text.encode_utf16().collect();
        if wide.is_empty() {
            continue;
        }
        let layout: IDWriteTextLayout =
            match unsafe { dwrite.CreateTextLayout(&wide, format, inner_w, line_h) } {
                Ok(l) => l,
                Err(_) => continue,
            };
        unsafe {
            let _ = layout.SetTextAlignment(dwrite_alignment);
            let _ = layout.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        }
        apply_cell_inline_runs(&layout, &line.text, &line.inline_runs);
        unsafe {
            ctx.DrawTextLayout(
                D2D_POINT_2F {
                    x: rect.left + TABLE_CELL_PAD_DIP,
                    y: rect.top + line_index as f32 * line_h,
                },
                &layout,
                brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }
    }
    unsafe {
        ctx.PopAxisAlignedClip();
    }
}

fn pick_text_brush<'a>(
    cell: &TableCellLayout,
    brushes: &'a TableVisualBrushes<'a>,
) -> &'a ID2D1SolidColorBrush {
    if cell.is_formula {
        if cell.display_text.starts_with('#') {
            brushes.formula_error
        } else {
            brushes.formula_value
        }
    } else {
        brushes.text_fg
    }
}

fn draw_cell_border(ctx: &ID2D1DeviceContext, rect: &D2D_RECT_F, brush: &ID2D1SolidColorBrush) {
    unsafe {
        ctx.DrawRectangle(rect, brush, TABLE_BORDER_STROKE_DIP, None);
    }
}

/// Apply Phase-B inline style runs onto a fresh cell text layout.
///
/// Cell inline runs are UTF-8 byte ranges indexing into `display_text`
/// (the post-marker-strip cell content). DirectWrite addresses runs by
/// UTF-16 code-unit offsets, so each range is translated through the
/// display text before being passed to `SetFontWeight` / `SetFontStyle`
/// / `SetStrikethrough` / `SetUnderline`. Code runs are left to the
/// caller's brush selection (no inline brush change here); the
/// monospaced typographic distinction stays a Phase-B-follow-up.
fn apply_cell_inline_runs(
    layout: &IDWriteTextLayout,
    display_text: &str,
    runs: &[(std::ops::Range<u32>, SpanStyle)],
) {
    if runs.is_empty() {
        return;
    }
    for (range, style) in runs {
        let start_byte = range.start as usize;
        let end_byte = (range.end as usize).min(display_text.len());
        if end_byte <= start_byte {
            continue;
        }
        let start_utf16 =
            crate::text_helpers::utf8_byte_to_utf16_index(display_text, start_byte) as u32;
        let end_utf16 =
            crate::text_helpers::utf8_byte_to_utf16_index(display_text, end_byte) as u32;
        let length = end_utf16.saturating_sub(start_utf16);
        if length == 0 {
            continue;
        }
        let dwrite_range = DWRITE_TEXT_RANGE {
            startPosition: start_utf16,
            length,
        };
        unsafe {
            if style.bold {
                let _ = layout.SetFontWeight(DWRITE_FONT_WEIGHT_BOLD, dwrite_range);
            } else {
                let _ = layout.SetFontWeight(DWRITE_FONT_WEIGHT_NORMAL, dwrite_range);
            }
            if style.italic {
                let _ = layout.SetFontStyle(DWRITE_FONT_STYLE_ITALIC, dwrite_range);
            } else {
                let _ = layout.SetFontStyle(DWRITE_FONT_STYLE_NORMAL, dwrite_range);
            }
            if style.strikethrough {
                let _ = layout.SetStrikethrough(true, dwrite_range);
            }
            if style.underline || matches!(style.role, SpanRole::Link) {
                let _ = layout.SetUnderline(true, dwrite_range);
            }
        }
    }
}

/// `true` when any `TableLayout` in `layouts` covers `block_range`.
/// Used by the F4 swap painter to skip tables that the visual layer is
/// already drawing — otherwise both would render the same eval value
/// at slightly different positions.
#[must_use]
pub(crate) fn block_has_visual_layout(
    block_range: &std::ops::Range<usize>,
    layouts: &[TableLayout],
) -> bool {
    layouts.iter().any(|l| l.block_range == *block_range)
}

#[cfg(test)]
mod tests;
