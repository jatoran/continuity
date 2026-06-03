//! Single spectator-pane body paint pass.

use continuity_layout::{FontStateId, LayoutCache};
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Graphics::Direct2D::Common::{D2D_POINT_2F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_DRAW_TEXT_OPTIONS_NONE,
};
use windows::Win32::Graphics::DirectWrite::{IDWriteFactory, IDWriteTextFormat};

use super::table_chrome::{paint_spectator_table_chrome, SpectatorTableChrome};
use super::{geometry, outline, PaneBodyBrushes};
use crate::chrome_line_numbers::paint_line_number_gutter;
use crate::display_projection::FrameDisplay;
use crate::inline_color_paint::{
    paint_inline_color_backgrounds_spec, paint_inline_color_foregrounds_spec,
};
use crate::params::{PaneBodyDraw, ViewOptionsDraw};
use crate::scroll_placeholder::{compute_unrealized_strips, paint_scroll_placeholder_strips};
use crate::table_formula_paint::{paint_table_overrides_spec, TableFormulaBrushes};
use crate::table_paint::TableVisualBrushes;
use crate::text_helpers::{build_key_for_spec, ensure_line_layout_for_spec};
use crate::text_role_effects::apply_role_drawing_effects;
use crate::Error;

/// Paint a single non-focused pane body inside its rect.
///
/// `font_state` and `format` are the window-level text format the
/// renderer is already using for the focused pane — we share them so
/// the layout cache doesn't fragment by font.
///
/// # Errors
///
/// Returns [`Error::Graphics`] when an underlying D2D/DWrite call fails.
///
/// # Safety
///
/// Caller must wrap this in a `BeginDraw`/`EndDraw` block. The function
/// installs and reverts its own translate so the caller can keep using
/// whatever transform was active before.
#[allow(clippy::too_many_arguments)]
pub(super) unsafe fn paint_pane_body(
    ctx: &ID2D1DeviceContext,
    factory: &IDWriteFactory,
    cache: &mut LayoutCache,
    body: &PaneBodyDraw<'_>,
    view_options: &ViewOptionsDraw<'_>,
    format: &IDWriteTextFormat,
    font_state: FontStateId,
    base_font_size_dip: f32,
    column_advance: f32,
    tab_advance: f32,
    line_height: f32,
    brushes: &PaneBodyBrushes<'_>,
) -> Result<(), Error> {
    let (rx, ry, rw, rh) = body.rect;
    if rw <= 0.0 || rh <= 0.0 {
        return Ok(());
    }
    let clip = D2D_RECT_F {
        left: rx,
        top: ry,
        right: rx + rw,
        bottom: ry + rh,
    };
    ctx.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
    ctx.FillRectangle(&clip, brushes.bg);
    let translate = Matrix3x2 {
        M11: 1.0,
        M12: 0.0,
        M21: 0.0,
        M22: 1.0,
        M31: rx,
        M32: ry,
    };
    ctx.SetTransform(&translate);

    let view = body.view;
    let viewport_h = view.viewport_height_dip.max(rh);
    let line_height = line_height.max(1.0);
    let margins = geometry::spectator_content_margins_with_right_edge(
        view_options.line_numbers,
        rw,
        base_font_size_dip,
        body.rope.len_lines(),
        body.minimap,
        body.show_outline_sidebar,
        view_options.outline_sidebar_width_dip,
    );
    let max_layout_width = f32::INFINITY;
    let wrap_width_dip_key = 0;
    let scroll_y = view.scroll_y_dip;

    let caret_bytes: Vec<usize> = body
        .selections
        .iter()
        .map(|s| {
            let line = s.head.line as usize;
            let line_start = if line < body.rope.len_lines() {
                body.rope.line_to_byte(line)
            } else {
                body.rope.len_bytes()
            };
            line_start + s.head.byte_in_line as usize
        })
        .collect();
    let selection_ranges: Vec<(usize, usize)> = body
        .selections
        .iter()
        .map(|s| {
            let to_abs = |p: continuity_text::Position| {
                let line = p.line as usize;
                let line_start = if line < body.rope.len_lines() {
                    body.rope.line_to_byte(line)
                } else {
                    body.rope.len_bytes()
                };
                line_start + p.byte_in_line as usize
            };
            let head = to_abs(s.head);
            let anchor = to_abs(s.anchor);
            if head <= anchor {
                (head, anchor)
            } else {
                (anchor, head)
            }
        })
        .collect();
    let owned_frame_display: Option<FrameDisplay> = if body.frame_display.is_none() {
        let wrap_width_dip = if view.soft_wrap {
            geometry::spectator_body_text_width_with_right_edge_for_line_count_dip(
                rw,
                base_font_size_dip,
                view_options.line_numbers,
                body.rope.len_lines(),
                body.minimap,
                body.show_outline_sidebar,
                view_options.outline_sidebar_width_dip,
            )
            .round()
            .max(0.0) as u32
        } else {
            0
        };
        Some(FrameDisplay::build(
            body.rope,
            0,
            body.decorations,
            &caret_bytes,
            wrap_width_dip,
            column_advance.max(1.0),
        ))
    } else {
        None
    };
    let frame_display: &FrameDisplay = match body.frame_display {
        Some(fd) => fd,
        None => owned_frame_display
            .as_ref()
            .expect("owned_frame_display built when body.frame_display is None"),
    };

    let table_visual_brushes = TableVisualBrushes {
        body_bg: brushes.bg,
        header_bg: brushes.table_header_bg,
        alignment_bg: brushes.table_alignment_bg,
        border: brushes.table_border,
        text_fg: brushes.fg,
        formula_value: brushes.formula_value,
        formula_error: brushes.formula_error,
    };
    let table_formula_brushes = TableFormulaBrushes {
        bg: brushes.bg,
        value: brushes.formula_value,
        error: brushes.formula_error,
    };
    let total_display_rows = frame_display.display_line_count() as i64;
    let first_visible = ((scroll_y / line_height).floor() as i64).max(0) as u32;
    let last_visible = ((((scroll_y + viewport_h) / line_height).ceil() as i64) + 1)
        .clamp(0, total_display_rows) as u32;
    let placeholder_strips = compute_unrealized_strips(
        frame_display.realized_row_range(),
        first_visible..last_visible,
        line_height,
        scroll_y,
    );
    if !placeholder_strips.is_empty() {
        paint_scroll_placeholder_strips(
            ctx,
            &placeholder_strips,
            0.0,
            (rw - margins.right).max(0.0),
            brushes.placeholder,
        );
    }
    if let Some(decorations) = body.decorations {
        crate::decoration_paint::paint_block_backgrounds(
            ctx,
            body.rope,
            frame_display,
            decorations,
            brushes.code_panel,
            brushes.code_panel_header,
            brushes.blockquote_bar,
            line_height,
            (rw - margins.right).max(margins.left),
            scroll_y,
            column_advance.max(base_font_size_dip * 0.6),
            margins.left,
        );
        let body_width_dip = (rw - margins.left - margins.right).max(1.0);
        crate::decoration_paint::paint_horizontal_rules(
            ctx,
            body.rope,
            frame_display,
            decorations,
            body.selections,
            brushes.hr,
            line_height,
            margins.left,
            body_width_dip,
            scroll_y,
            view_options.render_divider,
        );
    }
    for display_row in first_visible..last_visible {
        let Some(spec) = frame_display.display_line_by_index(display_row) else {
            continue;
        };
        let source_line = spec.source_line.raw() as usize;
        let leading_dip = if spec.is_wrap_continuation {
            FrameDisplay::leading_whitespace_advance_dip(
                body.rope,
                source_line,
                column_advance,
                tab_advance,
            )
        } else {
            0.0
        };
        let key = build_key_for_spec(spec, body.document, font_state, wrap_width_dip_key);
        ensure_line_layout_for_spec(
            cache,
            factory,
            spec,
            key,
            format,
            max_layout_width,
            base_font_size_dip,
            crate::DEFAULT_HEADING_SCALE,
        )?;
        let entry = cache.get(&key).expect("just inserted");
        let y = display_row as f32 * line_height - scroll_y;
        let line_translate = Matrix3x2 {
            M11: 1.0,
            M12: 0.0,
            M21: 0.0,
            M22: 1.0,
            M31: rx + margins.left + leading_dip,
            M32: ry + y,
        };
        ctx.SetTransform(&line_translate);
        paint_inline_color_backgrounds_spec(
            ctx,
            entry.layout,
            entry.text,
            spec,
            line_height,
            body.inline_color_spans,
            &caret_bytes,
            brushes.inline_highlight_bg,
        );
        if let Some(decorations) = body.decorations {
            crate::inline_code_paint::paint_inline_code_backgrounds_spec(
                ctx,
                entry.layout,
                entry.text,
                spec,
                line_height,
                &decorations.inlines,
                &caret_bytes,
                brushes.inline_code_bg,
                rx + margins.left + leading_dip,
                ry,
                y,
                None,
            );
        }
        apply_role_drawing_effects(entry.layout, spec.style_runs(), &brushes.text_roles);
        ctx.DrawTextLayout(
            D2D_POINT_2F { x: 0.0, y: 0.0 },
            entry.layout,
            brushes.fg,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
        );
        paint_inline_color_foregrounds_spec(
            ctx,
            factory,
            format,
            entry.layout,
            entry.text,
            spec,
            line_height,
            body.inline_color_spans,
            &caret_bytes,
        );
        paint_table_overrides_spec(
            ctx,
            factory,
            format,
            entry.layout,
            entry.text,
            spec,
            line_height,
            body.table_overrides,
            body.table_layouts,
            &table_formula_brushes,
        );
    }

    // Pipe-table chrome runs as a post-pass — after every body glyph —
    // so the cell fills erase the table's raw projected glyphs across
    // its full (possibly multi-line) vertical extent, mirroring the
    // focused pane's command-list replay. An inline per-row paint would
    // mask only a tall row's first display row, leaving its soft-wrap
    // continuation glyphs bleeding over the cell grid below. See
    // `super::table_chrome`.
    paint_spectator_table_chrome(
        ctx,
        factory,
        format,
        &SpectatorTableChrome {
            frame_display,
            table_layouts: body.table_layouts,
            brushes: &table_visual_brushes,
            caret_bytes: &caret_bytes,
            selection_ranges: &selection_ranges,
            outline_brush: brushes.table_active_cell_outline,
            caret_brush: brushes.fg,
            origin: (rx + margins.left, ry),
            line_height,
            scroll_y,
            column_advance,
            visible_rows: first_visible..last_visible,
        },
    );

    ctx.SetTransform(&translate);
    if view_options.line_numbers {
        let _ = paint_line_number_gutter(
            ctx,
            factory,
            format,
            body.rope,
            body.selections,
            line_height,
            scroll_y,
            rh,
            0,
            0,
            brushes.line_number,
            brushes.line_number_active,
            view_options.gutter_caret_line_only,
            view_options.relative_line_numbers,
            &[],
            Some(frame_display),
            false,
        );
    }

    if body.minimap {
        let outline_inset = if body.show_outline_sidebar {
            view_options.outline_sidebar_width_dip.max(0.0)
        } else {
            0.0
        };
        let layout = crate::minimap::compute_minimap_layout(
            (0.0, 0.0, rw, rh),
            scroll_y,
            line_height,
            body.rope.len_lines().max(1) as u64,
            outline_inset,
        );
        let _ = crate::minimap_paint::paint_minimap_scaled(
            ctx,
            factory,
            format,
            body.rope,
            &layout,
            brushes.minimap_colors,
        );
    }

    if body.show_outline_sidebar {
        let entries = outline::compute_spectator_outline_entries(body);
        let data = crate::outline::OutlineData {
            entries: &entries,
            current_index: None,
            colors: crate::outline::OutlineColors::default(),
            width_dip: view_options.outline_sidebar_width_dip,
            font_size_dip: base_font_size_dip,
            scroll_offset_dip: 0.0,
        };
        let _ = crate::outline_paint::paint_outline(
            ctx,
            factory,
            format,
            &data,
            (0.0, 0.0, rw, rh),
            brushes.outline_bg,
            brushes.outline_fg,
            brushes.outline_fg_active,
            brushes.outline_separator,
        );
    }

    ctx.PopAxisAlignedClip();
    let identity = Matrix3x2 {
        M11: 1.0,
        M12: 0.0,
        M21: 0.0,
        M22: 1.0,
        M31: 0.0,
        M32: 0.0,
    };
    ctx.SetTransform(&identity);
    Ok(())
}
