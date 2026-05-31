//! Spectator-pane pipe-table chrome post-pass.
//!
//! The focused pane paints table chrome through the retained
//! command-list cache ([`crate::table_chrome_cache`]), replayed as a
//! single masked image *after* the body glyph pass
//! ([`crate::renderer_table_chrome::replay_visible_tables`]). Spectator
//! panes have no such cache; this module paints the equivalent chrome
//! directly — but, crucially, as a post-pass that runs *after* the
//! per-display-row body-text loop in [`super::body::paint_pane_body`].
//!
//! Painting after the body text is what keeps a multi-line (`<br>` /
//! wrapped) table row correct. A table's source line projects to
//! visible body glyphs (pipes hidden, the trimmed cell text left at the
//! wrong x); when soft-wrap splits that line across several display rows
//! the continuation rows carry body glyphs too. An inline, per-row
//! chrome paint masks only the table's first display row, so those
//! continuation glyphs bleed over the cell grid below. Deferring the
//! chrome to a single pass lets the `body_bg` cell fills erase every
//! body glyph inside the table's vertical extent, exactly as the
//! focused pane's command-list replay does.
//!
//! Each table is anchored at its first source line's display row and
//! every row is stacked by
//! [`TableLayout::display_row_offset_within_table`] — the same
//! cumulative-offset math
//! [`crate::table_chrome_cache::record_table_chrome`] uses — so a tall
//! row pushes the rows below it down by its full height and the chrome
//! stays aligned even when the row's top has scrolled above the
//! viewport.
//!
//! Thread ownership: UI thread (caller owns the `ID2D1DeviceContext`).

use std::ops::Range;

use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Graphics::Direct2D::{ID2D1DeviceContext, ID2D1SolidColorBrush};
use windows::Win32::Graphics::DirectWrite::{IDWriteFactory, IDWriteTextFormat};

use crate::display_projection::FrameDisplay;
use crate::table_layout::TableLayout;
use crate::table_paint::{
    paint_active_cell_outline_line, paint_table_visual_line, TableLinePlacement, TableVisualBrushes,
};

/// Per-frame inputs for the spectator table-chrome post-pass. Bundled
/// so the entry point stays within clippy's argument cap.
pub(super) struct SpectatorTableChrome<'a> {
    /// Spectator's display projection — maps each table's first source
    /// line to its absolute display row.
    pub frame_display: &'a FrameDisplay,
    /// Visual layouts for every pipe-table in this pane.
    pub table_layouts: &'a [TableLayout],
    /// Cell fills / borders / header + alignment backgrounds / text.
    pub brushes: &'a TableVisualBrushes<'a>,
    /// Absolute caret byte offsets for the active-cell outline.
    pub caret_bytes: &'a [usize],
    /// Ordered selection byte ranges for the cell-selected fill.
    pub selection_ranges: &'a [(usize, usize)],
    /// Active-cell outline / selected-fill brush.
    pub outline_brush: &'a ID2D1SolidColorBrush,
    /// In-cell caret-bar brush (body foreground).
    pub caret_brush: &'a ID2D1SolidColorBrush,
    /// `(body-origin x after the left margin, body-origin y)` in screen
    /// DIPs — the same base the body-text loop translated by.
    pub origin: (f32, f32),
    /// Logical line height in DIPs.
    pub line_height: f32,
    /// Vertical scroll offset in DIPs.
    pub scroll_y: f32,
    /// Monospace column advance — positions the in-cell caret bar.
    pub column_advance: f32,
    /// Absolute display-row range the body loop painted. Rows whose
    /// vertical span misses it are culled; the pane clip would clip
    /// them anyway, but the cull keeps a long off-screen table off the
    /// per-row DirectWrite path.
    pub visible_rows: Range<u32>,
}

/// Paint every pipe-table's chrome for one spectator pane on top of the
/// already-painted body text. No-op when the pane has no tables.
///
/// Installs its own per-row transform; the caller restores the body
/// transform afterwards (as `paint_pane_body` does before the gutter
/// pass). Caller owns the surrounding `BeginDraw` block.
pub(super) fn paint_spectator_table_chrome(
    ctx: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    chrome: &SpectatorTableChrome<'_>,
) {
    if chrome.table_layouts.is_empty() {
        return;
    }
    let has_carets = !chrome.caret_bytes.is_empty() || !chrome.selection_ranges.is_empty();
    for layout in chrome.table_layouts {
        for row in layout.first_source_line..=layout.last_source_line {
            // Anchor each row at the frame's *actual* display position
            // and span its *actual* projected row count rather than
            // deriving them from `row_height`. The promoted frame after
            // a focus switch (or any raw-table-line soft-wrap) can
            // allocate a source line more — or differently placed —
            // display rows than the cell-wrap count reserves; following
            // the frame keeps the chrome tiled over the exact rows the
            // body-text loop just painted, so it can never misalign or
            // leave wrap-continuation glyphs unmasked.
            let row_display = chrome
                .frame_display
                .first_display_line_index_for_source(row as usize);
            let frame_rows = chrome
                .frame_display
                .display_line_count_for_source(row as usize)
                .max(1);
            // Cull rows whose multi-line span sits wholly above or below
            // the painted viewport.
            if row_display >= chrome.visible_rows.end
                || row_display + frame_rows <= chrome.visible_rows.start
            {
                continue;
            }
            let y = chrome.origin.1 + row_display as f32 * chrome.line_height - chrome.scroll_y;
            let translate = Matrix3x2 {
                M11: 1.0,
                M12: 0.0,
                M21: 0.0,
                M22: 1.0,
                M31: chrome.origin.0,
                M32: y,
            };
            unsafe {
                ctx.SetTransform(&translate);
            }
            paint_table_visual_line(
                ctx,
                dwrite,
                format,
                std::slice::from_ref(layout),
                TableLinePlacement {
                    source_line: row,
                    row_display_rows: frame_rows,
                    line_height_dip: chrome.line_height,
                    x_origin_dip: 0.0,
                },
                chrome.brushes,
            );
            if has_carets {
                paint_active_cell_outline_line(
                    ctx,
                    dwrite,
                    format,
                    std::slice::from_ref(layout),
                    TableLinePlacement {
                        source_line: row,
                        row_display_rows: frame_rows,
                        line_height_dip: chrome.line_height,
                        x_origin_dip: 0.0,
                    },
                    chrome.caret_bytes,
                    chrome.selection_ranges,
                    chrome.column_advance,
                    chrome.outline_brush,
                    chrome.caret_brush,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use continuity_decorate::TableAlignment;
    use continuity_display_map::{ImageRowReservation, SourceLine};
    use ropey::Rope;

    use crate::display_projection::FrameDisplay;
    use crate::table_layout::TableLayout;

    /// A table whose third row (source line 2) occupies two display
    /// rows; every other row is a single row. Mirrors the geometry the
    /// spectator post-pass tiles against.
    fn multiline_layout() -> TableLayout {
        TableLayout {
            block_range: 0..30,
            first_source_line: 0,
            last_source_line: 3,
            col_widths_dip: vec![64.0, 64.0],
            col_alignments: vec![TableAlignment::Left, TableAlignment::Left],
            cells: Vec::new(),
            alignment_row_source_line: Some(1),
            total_width_dip: 128.0,
            row_display_rows: vec![1, 1, 2, 1],
            wrap_cells: true,
        }
    }

    /// The post-pass anchors each row at the frame's
    /// `first_display_line_index_for_source` and spans the frame's
    /// `display_line_count_for_source`. Those two must tile
    /// contiguously — row `n+1` starts exactly where row `n`'s span
    /// ends — so consecutive cell rects abut without a gap or overlap,
    /// and the frame's per-row count must be at least the cell-wrap line
    /// count so the cell text always fits inside the masked rect.
    #[test]
    fn frame_rows_tile_contiguously_with_reservation() {
        let layout = multiline_layout();
        // Lines 0..=3 are the table; line 4 ("after") sits below it.
        let rope = Rope::from_str("header\nalign\ntall\nbody2\nafter\n");
        // The tall row reserves a second display row, exactly as
        // `table_row_reservations` would emit for `row_display_rows[2]`.
        let reservations = [ImageRowReservation {
            source_line: SourceLine(2),
            reserved_display_rows: 2,
        }];
        let frame =
            FrameDisplay::build_with_options(&rope, 1, None, &[], &[], &reservations, 0, 8.0);

        for row in layout.first_source_line..=layout.last_source_line {
            let row_display = frame.first_display_line_index_for_source(row as usize);
            let frame_rows = frame.display_line_count_for_source(row as usize).max(1);
            // The masked rect must cover at least every cell-wrap line.
            assert!(
                frame_rows >= layout.row_height(row),
                "row {row}: frame span {frame_rows} < cell-wrap height {}",
                layout.row_height(row),
            );
            // The next source line begins exactly where this row's span
            // ends — no gap, no overlap.
            let next = frame.first_display_line_index_for_source(row as usize + 1);
            assert_eq!(
                row_display + frame_rows,
                next,
                "row {row}: span [{row_display}, {}) does not abut next row at {next}",
                row_display + frame_rows,
            );
        }
    }

    /// The focus-switch / soft-wrap regression: when the raw table line
    /// wraps to more display rows than the cell reserves, the post-pass
    /// must span the frame's full count — not the smaller `row_height`
    /// — or the unmasked wrap-continuation glyphs bleed over the cell
    /// grid. Here a long line with soft-wrap on and no reservation
    /// occupies several display rows while a single-row table layout
    /// would otherwise mask only one.
    #[test]
    fn frame_span_exceeds_cell_wrap_height_when_line_soft_wraps() {
        let rope = Rope::from_str("aaaa bbbb cccc dddd eeee ffff gggg hhhh\nafter\n");
        // Narrow wrap width forces the first line across multiple rows;
        // no reservation is supplied, so the count comes purely from the
        // natural soft-wrap the way a promoted focused frame would carry
        // it.
        let frame = FrameDisplay::build_with_options(&rope, 1, None, &[], &[], &[], 40, 8.0);
        let frame_rows = frame.display_line_count_for_source(0).max(1);
        assert!(
            frame_rows > 1,
            "expected the long line to soft-wrap to >1 display rows, got {frame_rows}",
        );
        // A single-row cell layout would mask only one row; the post-pass
        // instead spans `frame_rows`, so the line after the table still
        // starts exactly below the full wrapped extent.
        assert_eq!(frame.first_display_line_index_for_source(1), frame_rows);
    }
}
