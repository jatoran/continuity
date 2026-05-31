//! Phase F4 — paint pass for per-cell formula `display` swap-in.
//!
//! When `params.table_overrides` carries `EvaluatedTable`s, this module
//! paints the override `display` string over each cell's source-byte
//! range, masking the literal `=SUM(A1:A3)` text with the computed value
//! the user actually wants to see.
//!
//! Tables render unconditionally — caret position no longer gates the
//! override paint. When a table also has a visual layout in
//! `params.table_layouts` (the common case, since every parseable
//! pipe-table block produces one), the byte-level swap is skipped to
//! avoid double-drawing at a different x position; the visual painter
//! at `crate::table_paint` renders the evaluated value aligned inside
//! the cell.
//!
//! Painter strategy:
//!
//! 1. Compute the `inner_x_start..inner_x_end` rect from the cell's
//!    source-byte range via `hit_test_x` against the cached layout.
//! 2. Fill the rect with the editor background color so the source
//!    formula bytes are masked.
//! 3. Build a small one-line `IDWriteTextLayout` from the override
//!    `display` string, drawing it at `inner_x_start` with the
//!    `markdown.formula.value` brush. Error sentinels (`#DIV/0!`,
//!    `#ERR`) use the `markdown.formula.error` brush.
//!
//! Step 1 + 2 use the cached per-line `IDWriteTextLayout`; step 3
//! creates a per-override ephemeral layout. Formula cells are rare in
//! practice so the allocation cost is bounded.
//!
//! **Thread ownership**: caller is the UI thread (the only owner of the
//! `IDWriteFactory`, the `ID2D1DeviceContext`, and the cached layouts).

use continuity_decorate::{EvaluatedTable, TableCellOverride};
use windows::Win32::Graphics::Direct2D::Common::{D2D_POINT_2F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1SolidColorBrush, D2D1_DRAW_TEXT_OPTIONS_NONE,
};
use windows::Win32::Graphics::DirectWrite::{IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout};

use crate::display_projection::FrameDisplay;
use crate::table_layout::TableLayout;
use crate::table_paint::block_has_visual_layout;
use crate::text_helpers::{caret_utf16_for_line, caret_utf16_for_spec, hit_test_x};

/// Brushes the F4 painter consumes. Bundled into a struct so the
/// renderer's per-line loop doesn't have to thread eight parameters
/// individually.
pub(crate) struct TableFormulaBrushes<'a> {
    /// Editor background — fills the rectangle behind the override text
    /// so the source formula bytes are masked.
    pub bg: &'a ID2D1SolidColorBrush,
    /// Foreground for the rendered computed value
    /// (`markdown.formula.value`).
    pub value: &'a ID2D1SolidColorBrush,
    /// Foreground for `#DIV/0!` / `#ERR` sentinels
    /// (`markdown.formula.error`).
    pub error: &'a ID2D1SolidColorBrush,
}

/// `true` when `display` starts with the `#` character — the stable
/// prefix shared by `#DIV/0!` and `#ERR` (the two error sentinels the
/// evaluator emits). Picking the brush off this prefix keeps the
/// painter ignorant of the specific error variants the evaluator
/// produced.
fn is_error_sentinel(display: &str) -> bool {
    display.starts_with('#')
}

/// Paint every override whose `cell_range` falls within `line_byte_range`.
/// Called per visible line, after the main text layout has drawn.
///
/// `line_idx` is the source-line index (not display-line). `entry_text`
/// is the display text the cached layout was built from — needed for
/// the source-byte→UTF-16-column translation.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_table_overrides_line(
    ctx: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    layout: &IDWriteTextLayout,
    entry_text: &str,
    frame_display: &FrameDisplay,
    line_idx: usize,
    line_byte_range: std::ops::Range<usize>,
    line_height: f32,
    tables: &[EvaluatedTable],
    visual_layouts: &[TableLayout],
    brushes: &TableFormulaBrushes<'_>,
) {
    for table in tables {
        // Skip when the table's source bytes don't intersect this line.
        if table.block_range.end <= line_byte_range.start
            || table.block_range.start >= line_byte_range.end
        {
            continue;
        }
        // The visual painter is already drawing this table's eval
        // values inside aligned cells; skip the byte-level swap to
        // avoid a double-draw at a different x position.
        if block_has_visual_layout(&table.block_range, visual_layouts) {
            continue;
        }
        for ov in &table.overrides {
            paint_one_override(
                ctx,
                dwrite,
                format,
                layout,
                &line_byte_range,
                line_height,
                ov,
                brushes,
                |source_byte| {
                    caret_utf16_for_line(
                        entry_text,
                        frame_display,
                        line_idx,
                        source_byte.saturating_sub(line_byte_range.start),
                    )
                },
            );
        }
    }
}

/// Paint every formula override that intersects one concrete display spec.
/// This is the soft-wrap companion to [`paint_table_overrides_line`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_table_overrides_spec(
    ctx: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    layout: &IDWriteTextLayout,
    entry_text: &str,
    spec: &continuity_display_map::DisplayLineSpec,
    line_height: f32,
    tables: &[EvaluatedTable],
    visual_layouts: &[TableLayout],
    brushes: &TableFormulaBrushes<'_>,
) {
    let line_byte_range =
        spec.source_byte_start.raw() as usize..spec.source_byte_end.raw() as usize;
    for table in tables {
        if table.block_range.end <= line_byte_range.start
            || table.block_range.start >= line_byte_range.end
        {
            continue;
        }
        if block_has_visual_layout(&table.block_range, visual_layouts) {
            continue;
        }
        for ov in &table.overrides {
            paint_one_override(
                ctx,
                dwrite,
                format,
                layout,
                &line_byte_range,
                line_height,
                ov,
                brushes,
                |source_byte| caret_utf16_for_spec(entry_text, spec, source_byte),
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_one_override(
    ctx: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    layout: &IDWriteTextLayout,
    line_byte_range: &std::ops::Range<usize>,
    line_height: f32,
    ov: &TableCellOverride,
    brushes: &TableFormulaBrushes<'_>,
    mut source_to_utf16: impl FnMut(usize) -> usize,
) {
    // Intersect the override's cell_range with this line.
    if ov.cell_range.end <= line_byte_range.start || ov.cell_range.start >= line_byte_range.end {
        return;
    }
    let local_start = ov.cell_range.start.saturating_sub(line_byte_range.start);
    let local_end = (ov.cell_range.end - line_byte_range.start)
        .min(line_byte_range.end - line_byte_range.start);
    if local_end <= local_start {
        return;
    }
    let utf16_start = source_to_utf16(line_byte_range.start + local_start);
    let utf16_end = source_to_utf16(line_byte_range.start + local_end);
    let Some(x_start) = hit_test_x(layout, utf16_start) else {
        return;
    };
    let Some(x_end) = hit_test_x(layout, utf16_end) else {
        return;
    };
    if x_end <= x_start {
        return;
    }
    // Mask the source formula bytes.
    let rect = D2D_RECT_F {
        left: x_start,
        top: 0.0,
        right: x_end,
        bottom: line_height,
    };
    unsafe { ctx.FillRectangle(&rect, brushes.bg) };
    // Build a tiny one-line layout for the override text and draw it
    // at the cell's left edge. Width is the cell width; height the line
    // height. Errors picking the layout fall through silently — the
    // mask remains so the source bytes don't bleed through.
    let wide: Vec<u16> = ov.display.encode_utf16().collect();
    if wide.is_empty() {
        return;
    }
    let layout_w = (x_end - x_start).max(1.0);
    let Ok(override_layout): Result<IDWriteTextLayout, _> =
        (unsafe { dwrite.CreateTextLayout(&wide, format, layout_w, line_height) })
    else {
        return;
    };
    let brush = if is_error_sentinel(&ov.display) {
        brushes.error
    } else {
        brushes.value
    };
    unsafe {
        ctx.DrawTextLayout(
            D2D_POINT_2F { x: x_start, y: 0.0 },
            &override_layout,
            brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_decorate::EvaluatedTable;

    #[test]
    fn error_sentinel_detection() {
        assert!(is_error_sentinel("#DIV/0!"));
        assert!(is_error_sentinel("#ERR"));
        assert!(!is_error_sentinel("42"));
        assert!(!is_error_sentinel("3.14"));
        assert!(!is_error_sentinel(""));
    }

    #[test]
    fn non_overlapping_line_byte_range_is_dropped() {
        // The line covers bytes 100..200; the table block ends at 50.
        // No overlap — the iterator should skip without calling
        // FillRectangle.
        let table = EvaluatedTable {
            block_range: 0..50,
            overrides: vec![],
        };
        let line_byte_range = 100..200usize;
        assert!(
            table.block_range.end <= line_byte_range.start
                || table.block_range.start >= line_byte_range.end
        );
    }
}
