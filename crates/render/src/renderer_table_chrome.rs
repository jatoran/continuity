//! Per-frame orchestration for the [`crate::table_chrome_cache`].
//!
//! Extracted from [`crate::Renderer::draw_buffer_no_present`] so the
//! renderer file stays under the 600-line cap. Two entry points:
//!
//! - [`prepare_visible_tables`] runs **before** the outer `BeginDraw`.
//!   It walks `params.table_layouts`, culls to the viewport, and asks
//!   the cache to ensure each visible table's `ID2D1CommandList` is
//!   ready. Returns the visible-table plan plus partial stats
//!   (`fresh_count`, `record_us`).
//! - [`replay_visible_tables`] runs **after** the body glyph pass but
//!   before spell / focus-dim / chrome-post. It installs each table's
//!   screen-space transform and `DrawImage`s the cached list. Adds
//!   `replay_us` and `replay_count` to the plan's stats.
//!
//! Thread ownership: UI thread.

use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1RenderTarget, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{IDWriteFactory, IDWriteTextFormat};

use crate::chrome::ContentMargins;
use crate::chrome_command_list::{ChromeCommandListKey, ChromeRecordingGeometry};
use crate::params::{DrawParams, Rgba};
use crate::renderer::Renderer;
use crate::table_chrome_cache::{
    record_table_chrome, TableChromeCache, TableChromeKey, TableChromePathMode,
    TableChromePathStats, TableId,
};
use crate::table_layout::TableLayout;
use crate::table_paint::{paint_active_cell_outline_line, TableLinePlacement, TableVisualBrushes};
use crate::Error;

/// One visible table's identity and screen-space anchor for the
/// replay pass.
pub(crate) struct VisibleTableRef {
    /// Cache identity — passed back into `cache.replay`.
    pub(crate) table_id: TableId,
    /// Display-row index of the table's first source line. Multiplied
    /// by `line_height` and added to `body_origin.y - scroll_y` to
    /// position the cached image vertically.
    pub(crate) first_display_row: u32,
}

/// Per-frame plan for the table chrome pass.
pub(crate) struct TableChromeFramePlan {
    /// Visible-table references in `params.table_layouts` order.
    pub(crate) visible: Vec<VisibleTableRef>,
    /// Stats accumulated across prepare + replay.
    pub(crate) stats: TableChromePathStats,
}

impl TableChromeFramePlan {
    /// Empty plan — used when no tables are visible.
    fn empty() -> Self {
        Self {
            visible: Vec::new(),
            stats: TableChromePathStats::default(),
        }
    }
}

/// Walk `params.table_layouts`, cull to the viewport, and ensure each
/// visible table has a fresh-or-cached `ID2D1CommandList` in `cache`.
///
/// MUST be called **before** the renderer's outer `BeginDraw` — the
/// cache's inner `BeginDraw` would otherwise nest.
pub(crate) fn prepare_visible_tables(
    device_context: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    cache: &mut TableChromeCache,
    params: &DrawParams<'_>,
    line_height_dip: f32,
    scroll_y_dip: f32,
    viewport_h_dip: f32,
) -> Result<TableChromeFramePlan, Error> {
    if params.table_layouts.is_empty() {
        return Ok(TableChromeFramePlan::empty());
    }
    let mut plan = TableChromeFramePlan::empty();
    let frame_display = params.frame_display;
    let line_height = line_height_dip.max(1.0);
    let viewport_top = scroll_y_dip;
    let viewport_bottom = scroll_y_dip + viewport_h_dip.max(0.0);
    for layout in params.table_layouts {
        if !table_intersects_viewport(
            layout,
            frame_display,
            line_height,
            viewport_top,
            viewport_bottom,
        ) {
            continue;
        }
        let table_id = TableId::new(params.document, layout.block_range.start);
        let key = TableChromeKey::for_layout(
            layout,
            params.theme_revision,
            params.font_state.0,
            params.dpi_scale,
            line_height,
            params.base_font_size_dip,
        );
        let (mode, record_us) = cache.prepare(table_id, key, device_context, |ctx| {
            record_one_table(ctx, dwrite, params, layout, line_height)
        })?;
        let first_display_row =
            frame_display.first_display_line_index_for_source(layout.first_source_line as usize);
        plan.visible.push(VisibleTableRef {
            table_id,
            first_display_row,
        });
        plan.stats.tables_painted = plan.stats.tables_painted.saturating_add(1);
        match mode {
            TableChromePathMode::Fresh => {
                plan.stats.fresh_count = plan.stats.fresh_count.saturating_add(1);
                plan.stats.record_us = plan.stats.record_us.saturating_add(record_us);
            }
            TableChromePathMode::Replay => {
                plan.stats.replay_count = plan.stats.replay_count.saturating_add(1);
            }
        }
    }
    Ok(plan)
}

/// `true` when `layout`'s vertical extent in display-row space crosses
/// `[viewport_top, viewport_bottom]`. Tables whose every row sits
/// above or below the viewport are skipped so the cache's working set
/// stays bounded to what's visible.
fn table_intersects_viewport(
    layout: &TableLayout,
    frame_display: &crate::display_projection::FrameDisplay,
    line_height: f32,
    viewport_top: f32,
    viewport_bottom: f32,
) -> bool {
    let first_display =
        frame_display.first_display_line_index_for_source(layout.first_source_line as usize) as f32
            * line_height;
    // Phase F — the table's bottom is its first display row plus the
    // sum of every row's (possibly multi-line) height, so a tall table
    // is not culled while its lower rows are still on screen.
    let last_display = first_display + layout.total_display_rows() as f32 * line_height;
    last_display > viewport_top && first_display < viewport_bottom
}

fn record_one_table(
    device_context: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    params: &DrawParams<'_>,
    layout: &TableLayout,
    line_height_dip: f32,
) -> Result<(), Error> {
    use windows::core::Interface;
    let render_target: ID2D1RenderTarget = device_context.cast()?;
    let make_brush = |rgba: Rgba| -> Result<ID2D1SolidColorBrush, Error> {
        Ok(unsafe { render_target.CreateSolidColorBrush(&D2D1_COLOR_F::from(rgba), None)? })
    };
    let body_bg = make_brush(params.colors.bg)?;
    let header_bg = make_brush(params.markdown_colors.table_header_bg)?;
    let alignment_bg = make_brush(params.markdown_colors.table_alignment_bg)?;
    let border = make_brush(params.markdown_colors.table_border)?;
    let text_fg = make_brush(params.colors.fg)?;
    let formula_value = make_brush(params.markdown_colors.formula_value)?;
    let formula_error = make_brush(params.markdown_colors.formula_error)?;
    let brushes = TableVisualBrushes {
        body_bg: &body_bg,
        header_bg: &header_bg,
        alignment_bg: &alignment_bg,
        border: &border,
        text_fg: &text_fg,
        formula_value: &formula_value,
        formula_error: &formula_error,
    };
    record_table_chrome(
        device_context,
        dwrite,
        params.format,
        layout,
        line_height_dip,
        &brushes,
    );
    Ok(())
}

/// For every entry in `plan.visible`, install a per-table transform
/// (`body_origin + (margins_left, first_display_row * line_height -
/// scroll_y)`) and replay the cached command list. Restores the body
/// transform so subsequent painters (spell, focus dim, chrome-post)
/// continue in body-relative coords.
pub(crate) fn replay_visible_tables(
    device_context: &ID2D1DeviceContext,
    cache: &TableChromeCache,
    plan: &mut TableChromeFramePlan,
    body_origin: (f32, f32),
    margins_left: f32,
    line_height_dip: f32,
    scroll_y_dip: f32,
) -> Result<(), Error> {
    if plan.visible.is_empty() {
        restore_body_transform(device_context, body_origin);
        return Ok(());
    }
    for entry in &plan.visible {
        let translate = Matrix3x2 {
            M11: 1.0,
            M12: 0.0,
            M21: 0.0,
            M22: 1.0,
            M31: body_origin.0 + margins_left,
            M32: body_origin.1 + entry.first_display_row as f32 * line_height_dip - scroll_y_dip,
        };
        unsafe {
            device_context.SetTransform(&translate);
        }
        let replay_us = cache.replay(entry.table_id, device_context)?;
        plan.stats.replay_us = plan.stats.replay_us.saturating_add(replay_us);
    }
    restore_body_transform(device_context, body_origin);
    Ok(())
}

fn restore_body_transform(device_context: &ID2D1DeviceContext, body_origin: (f32, f32)) {
    let body_translate = Matrix3x2 {
        M11: 1.0,
        M12: 0.0,
        M21: 0.0,
        M22: 1.0,
        M31: body_origin.0,
        M32: body_origin.1,
    };
    unsafe {
        device_context.SetTransform(&body_translate);
    }
}

/// Scalar geometry inputs that
/// [`prepare_retained_chrome`] forwards to the two cache prep paths.
pub(crate) struct ChromePrepGeometry {
    pub body_translate: Matrix3x2,
    pub body_clip: D2D_RECT_F,
    pub margins: ContentMargins,
    pub viewport_w: f32,
    pub viewport_h: f32,
    pub editor_w: f32,
    pub line_height: f32,
    pub column_advance: f32,
    pub scroll_y: f32,
}

/// Prepare both retained-chrome caches (P14 static shell + P14.1 per-
/// table) before the renderer's outer `BeginDraw`. Returns the
/// per-table frame plan; the static-chrome stats are stamped on
/// `renderer.last_chrome_path_stats` later via the post-body pass.
pub(crate) fn prepare_retained_chrome(
    renderer: &Renderer,
    params: &DrawParams<'_>,
    geometry: ChromePrepGeometry,
) -> Result<TableChromeFramePlan, Error> {
    let chrome_geometry = ChromeRecordingGeometry {
        body_translate: geometry.body_translate,
        body_clip: geometry.body_clip,
        margins: geometry.margins,
        viewport_width_dip: geometry.viewport_w,
        viewport_height_dip: geometry.viewport_h,
        editor_width_dip: geometry.editor_w,
        line_height_dip: geometry.line_height,
        column_advance_dip: geometry.column_advance,
    };
    let chrome_key = ChromeCommandListKey::from_draw_params(params, &chrome_geometry);
    renderer.chrome_command_list.borrow_mut().prepare(
        &renderer.d2d_context,
        chrome_key,
        |device_context| {
            crate::chrome_command_list::record_static_chrome(
                device_context,
                params,
                chrome_geometry,
            )
        },
    )?;
    let mut cache = renderer.table_chrome_cache.borrow_mut();
    cache.begin_frame();
    prepare_visible_tables(
        &renderer.d2d_context,
        &renderer.dwrite_factory,
        &mut cache,
        params,
        geometry.line_height,
        geometry.scroll_y,
        geometry.viewport_h,
    )
}

/// Replay every visible table and stamp the resulting stats on
/// `renderer.last_table_chrome_stats`.
pub(crate) fn run_replay(
    renderer: &Renderer,
    plan: &mut TableChromeFramePlan,
    body_origin: (f32, f32),
    margins_left: f32,
    line_height: f32,
    scroll_y: f32,
) -> Result<(), Error> {
    let cache = renderer.table_chrome_cache.borrow();
    replay_visible_tables(
        &renderer.d2d_context,
        &cache,
        plan,
        body_origin,
        margins_left,
        line_height,
        scroll_y,
    )?;
    renderer.last_table_chrome_stats.set(plan.stats);
    Ok(())
}

/// Paint the active-cell outline for the focused pane.
///
/// Runs **after** [`run_replay`] so the outline draws on top of the
/// replayed chrome bitmap. For each table layout that has a caret
/// inside one of its cells, walks the source-line range and paints a
/// 2-DIP outline over the caret-containing cell. The per-line
/// transform is applied here (mirroring [`replay_visible_tables`]'s
/// per-table transform) and the body transform is restored at the end
/// so the post-body pipeline continues in body-relative coords.
///
/// No-op when no tables exist or no caret falls inside any table cell.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_focused_active_cell_outlines(
    device_context: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    table_layouts: &[TableLayout],
    frame_display: &crate::display_projection::FrameDisplay,
    rope: &ropey::Rope,
    selections: &[continuity_text::Selection],
    body_origin: (f32, f32),
    margins_left: f32,
    line_height_dip: f32,
    scroll_y_dip: f32,
    column_advance_dip: f32,
    outline_brush: &ID2D1SolidColorBrush,
    caret_brush: &ID2D1SolidColorBrush,
) {
    if table_layouts.is_empty() {
        return;
    }
    let (head_bytes, selection_ranges) = selection_bytes_for_outline(rope, selections);
    if head_bytes.is_empty() {
        return;
    }
    // Iterate selections × tables × cells rather than tables × rows ×
    // cells. For typical documents this is O(carets × cells_per_table)
    // instead of O(rows × cells_per_row²), which keeps the per-frame
    // cost flat as table size grows. Track which (layout_idx, cell_idx)
    // pairs we've already painted so two carets in the same cell don't
    // double-paint (and don't drive the chrome cache twice).
    let mut painted: Vec<(usize, usize)> = Vec::new();
    for (sel_idx, &(sel_start, sel_end)) in selection_ranges.iter().enumerate() {
        let head = head_bytes.get(sel_idx).copied().unwrap_or(sel_start);
        let probe_byte = head;
        for (layout_idx, layout) in table_layouts.iter().enumerate() {
            if probe_byte < layout.block_range.start || probe_byte >= layout.block_range.end {
                continue;
            }
            for (cell_idx, cell) in layout.cells.iter().enumerate() {
                if cell.is_alignment_row {
                    continue;
                }
                let head_in_cell = head >= cell.source_range.start && head <= cell.source_range.end;
                let covers = sel_start == cell.source_range.start
                    && sel_end == cell.source_range.end
                    && sel_start != sel_end;
                if !head_in_cell && !covers {
                    continue;
                }
                if painted.contains(&(layout_idx, cell_idx)) {
                    continue;
                }
                painted.push((layout_idx, cell_idx));
                let display_row =
                    frame_display.first_display_line_index_for_source(cell.source_line as usize);
                let translate = Matrix3x2 {
                    M11: 1.0,
                    M12: 0.0,
                    M21: 0.0,
                    M22: 1.0,
                    M31: body_origin.0 + margins_left,
                    M32: body_origin.1 + display_row as f32 * line_height_dip - scroll_y_dip,
                };
                unsafe {
                    device_context.SetTransform(&translate);
                }
                paint_active_cell_outline_line(
                    device_context,
                    dwrite,
                    format,
                    std::slice::from_ref(layout),
                    TableLinePlacement {
                        source_line: cell.source_line,
                        row_display_rows: layout.row_height(cell.source_line),
                        line_height_dip,
                        x_origin_dip: 0.0,
                    },
                    &head_bytes,
                    &selection_ranges,
                    column_advance_dip,
                    outline_brush,
                    caret_brush,
                );
                break;
            }
        }
    }
    restore_body_transform(device_context, body_origin);
}

/// For each selection, derive the absolute head byte (used to
/// position the in-cell caret) and the ordered `(start, end)` byte
/// pair (used to detect when a selection fully covers a cell). Both
/// vectors index `selections` in the same order.
fn selection_bytes_for_outline(
    rope: &ropey::Rope,
    selections: &[continuity_text::Selection],
) -> (Vec<usize>, Vec<(usize, usize)>) {
    let mut heads = Vec::with_capacity(selections.len());
    let mut ranges = Vec::with_capacity(selections.len());
    for sel in selections {
        let head = position_to_absolute_byte(rope, sel.head);
        let anchor = position_to_absolute_byte(rope, sel.anchor);
        let (start, end) = if head <= anchor {
            (head, anchor)
        } else {
            (anchor, head)
        };
        heads.push(head);
        ranges.push((start, end));
    }
    (heads, ranges)
}

fn position_to_absolute_byte(rope: &ropey::Rope, pos: continuity_text::Position) -> usize {
    let line = pos.line as usize;
    let line_start = if line < rope.len_lines() {
        rope.line_to_byte(line)
    } else {
        rope.len_bytes()
    };
    line_start + pos.byte_in_line as usize
}
