//! Body of [`crate::Renderer::draw_buffer_no_present`].
//!
//! Chrome-overlay sub-stage durations land in
//! [`crate::Renderer::last_chrome_overlay_breakdown`] for the UI
//! thread's `event:renderer_draw_stages` trace row.
//!
//! Thread ownership: UI thread.

use std::time::Instant;

use continuity_layout::LayoutCache;
use continuity_text::Selection;
use ropey::Rope;
use windows::core::Interface;
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1RenderTarget, ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_ALIASED,
};

use crate::chrome::{paint_current_line_highlight, paint_trailing_whitespace};
use crate::chrome_caret::caret_rect_for_shape;
use crate::decoration_paint::{paint_block_backgrounds, paint_horizontal_rules_pass};
use crate::params::{DrawParams, Rgba};
use crate::render_stats::chrome_overlay_breakdown::RendererChromeOverlayBreakdown;
use crate::renderer::Renderer;
use crate::Error;

mod minimap_pass;
mod timing;
use timing::elapsed_us;

/// Render one frame's draw commands without calling `Present`. Body of
/// [`Renderer::draw_buffer_no_present`] — kept here so the renderer
/// file stays under the conventions cap once the chrome-overlay
/// sub-stage timing wrappers land.
#[allow(clippy::too_many_lines)]
pub(crate) fn render_frame(
    renderer: &Renderer,
    rope: &Rope,
    selections: &[Selection],
    cache: &mut LayoutCache,
    params: &DrawParams<'_>,
) -> Result<(), Error> {
    let mut breakdown = RendererChromeOverlayBreakdown::default();
    let bg_d2d: D2D1_COLOR_F = params.colors.bg.into();
    let render_target: ID2D1RenderTarget = renderer.d2d_context.cast()?;
    let viewport_w = params.view.viewport_width_dip.max(1.0);
    let viewport_h = params.view.viewport_height_dip.max(1.0);
    let line_height = params.line_height.max(1.0);
    let total_lines = rope.len_lines().max(1);
    let margins = crate::chrome_centered::resolve_margins_for_line_count(
        &params.view_options,
        viewport_w,
        params.base_font_size_dip,
        total_lines,
    );
    let editor_w = (viewport_w - margins.left - margins.right).max(1.0);
    let max_layout_width = if params.view.soft_wrap {
        editor_w
    } else {
        f32::INFINITY
    };
    let wrap_width_dip = params.view.wrap_width_key();
    let scroll_y = params.view.scroll_y_dip;
    let first_visible = ((scroll_y / line_height).floor() as isize).max(0) as usize;
    let last_visible =
        (((scroll_y + viewport_h) / line_height).ceil() as usize + 1).min(total_lines);
    let column_advance = crate::text_metrics::measure_space_advance_dip(
        &renderer.dwrite_factory,
        params.format,
        params.base_font_size_dip,
    );
    let tab_advance = crate::text_metrics::measure_tab_advance_dip(
        &renderer.dwrite_factory,
        params.format,
        column_advance,
        params.view_options.tab_width,
    );
    let body_translate = Matrix3x2 {
        M11: 1.0,
        M12: 0.0,
        M21: 0.0,
        M22: 1.0,
        M31: params.body_origin.0,
        M32: params.body_origin.1,
    };
    let body_clip = D2D_RECT_F {
        left: 0.0,
        top: 0.0,
        right: (viewport_w - margins.right).max(0.0),
        bottom: viewport_h,
    };
    // Table-chrome plan prep counts as decoration overhead (the per-
    // table record/replay duration is reported on its own bucket via
    // `chrome_overlay_table_us`).
    let table_prep_started = Instant::now();
    let mut table_chrome_plan = crate::renderer_table_chrome::prepare_retained_chrome(
        renderer,
        params,
        crate::renderer_table_chrome::ChromePrepGeometry {
            body_translate,
            body_clip,
            margins,
            viewport_w,
            viewport_h,
            editor_w,
            line_height,
            column_advance,
            scroll_y,
        },
    )?;
    breakdown.decoration_us = breakdown
        .decoration_us
        .saturating_add(elapsed_us(table_prep_started));

    unsafe {
        renderer.d2d_context.BeginDraw();
        renderer.d2d_context.Clear(Some(&bg_d2d));

        renderer.d2d_context.SetTransform(&body_translate);
        renderer
            .d2d_context
            .PushAxisAlignedClip(&body_clip, D2D1_ANTIALIAS_MODE_ALIASED);

        let brushes_started = Instant::now();
        let mkb = |rgba: Rgba| -> Result<ID2D1SolidColorBrush, Error> {
            Ok(render_target.CreateSolidColorBrush(&D2D1_COLOR_F::from(rgba), None)?)
        };
        let bg_brush = render_target.CreateSolidColorBrush(&bg_d2d, None)?;
        let fg_brush = mkb(params.colors.fg)?;
        let text_role_brush_set = crate::text_role_effects::TextRoleBrushSet::new(
            &render_target,
            &params.markdown_colors,
            params.colors.fg,
        )?;
        let text_role_brushes = text_role_brush_set.refs();
        let caret_brush = mkb(params.colors.caret)?;
        let table_active_cell_brush = mkb(params.markdown_colors.table_active_cell_outline)?;
        let secondary_caret_brush = mkb(params.colors.secondary_caret)?;
        let selection_brush = mkb(params.colors.selection)?;
        let selected_line_brush = mkb(crate::line_bands::scaled_alpha(
            params.colors.selection,
            0.30,
        ))?;
        let search_match_brush = mkb(params.colors.search_match)?;
        let search_match_active_brush = mkb(params.colors.search_match_active)?;
        let code_panel_brush = mkb(params.markdown_colors.code_block_bg)?;
        let code_panel_header = crate::decoration_paint::compute_code_block_header_color(
            params.markdown_colors.code_block_bg,
        );
        let code_panel_header_brush = mkb(code_panel_header)?;
        let blockquote_brush = mkb(params.markdown_colors.blockquote_bar)?;
        let hr_brush = mkb(params.markdown_colors.hr)?;
        let inline_highlight_bg_brush = mkb(params.markdown_colors.inline_highlight_bg)?;
        let inline_code_bg_brush = mkb(params.markdown_colors.code_bg)?;
        let formula_value_brush = mkb(params.markdown_colors.formula_value)?;
        let formula_error_brush = mkb(params.markdown_colors.formula_error)?;
        let line_highlight_brush = mkb(params.colors.line_highlight)?;
        // §6a — caret line uses its own key (distinct from the hover band,
        // which still derives from `editor.line_highlight`).
        let caret_line_brush = mkb(params.colors.caret_line_highlight)?;
        let hover_line_brush = mkb(crate::line_bands::scaled_alpha(
            params.colors.line_highlight,
            0.42,
        ))?;
        let hover_gutter_brush = mkb(crate::line_bands::scaled_alpha(
            params.colors.line_highlight,
            0.62,
        ))?;
        let line_number_brush = mkb(params.colors.line_number)?;
        let line_number_active_brush = mkb(params.colors.line_number_active)?;
        let indent_guide_brush = mkb(params.colors.indent_guide)?;
        let indent_guide_active_brush = mkb(params.colors.indent_guide_active)?;
        breakdown.decoration_us = breakdown
            .decoration_us
            .saturating_add(elapsed_us(brushes_started));

        if params.view_options.current_line_highlight {
            let started = Instant::now();
            let fd = &params.frame_display;
            let display_rows = selections.first().map(|s| {
                let l = s.head.line as usize;
                (
                    fd.first_display_line_index_for_source(l),
                    fd.display_line_count_for_source(l),
                )
            });
            paint_current_line_highlight(
                &renderer.d2d_context,
                rope,
                selections,
                line_height,
                scroll_y,
                viewport_w,
                margins,
                &caret_line_brush,
                display_rows,
            );
            breakdown.selection_bars_us = breakdown
                .selection_bars_us
                .saturating_add(elapsed_us(started));
        }

        {
            let started = Instant::now();
            crate::line_bands::paint_line_bands(
                &renderer.d2d_context,
                selections,
                params.frame_display,
                params.line_hover,
                line_height,
                scroll_y,
                viewport_h,
                viewport_w,
                margins,
                if params.view_options.line_numbers {
                    crate::chrome::gutter_width_for_line_count(
                        params.base_font_size_dip,
                        total_lines,
                    )
                } else {
                    0.0
                },
                &selected_line_brush,
                &hover_line_brush,
                &hover_gutter_brush,
            );
            breakdown.selection_bars_us = breakdown
                .selection_bars_us
                .saturating_add(elapsed_us(started));
        }

        if params.view_options.trailing_whitespace {
            let started = Instant::now();
            let body_content_translate = Matrix3x2 {
                M31: body_translate.M31 + margins.left,
                ..body_translate
            };
            renderer.d2d_context.SetTransform(&body_content_translate);
            let zero_left = crate::chrome::ContentMargins {
                left: 0.0,
                right: margins.right,
            };
            paint_trailing_whitespace(
                &renderer.d2d_context,
                rope,
                line_height,
                scroll_y,
                zero_left,
                column_advance,
                first_visible,
                last_visible,
                &line_highlight_brush,
            );
            renderer.d2d_context.SetTransform(&body_translate);
            breakdown.selection_bars_us = breakdown
                .selection_bars_us
                .saturating_add(elapsed_us(started));
        }

        if let Some(decorations) = params.decorations {
            let started = Instant::now();
            let code_panel_column_advance = column_advance.max(params.base_font_size_dip * 0.6);
            paint_block_backgrounds(
                &renderer.d2d_context,
                rope,
                params.frame_display,
                decorations,
                &code_panel_brush,
                &code_panel_header_brush,
                &blockquote_brush,
                line_height,
                editor_w + margins.left,
                scroll_y,
                code_panel_column_advance,
                margins.left,
            );
            breakdown.block_backgrounds_us = elapsed_us(started);
        }

        let placeholder_started = Instant::now();
        let placeholder_visible =
            crate::renderer_scroll_placeholder::paint_scroll_placeholder_pass(
                &renderer.d2d_context,
                &render_target,
                params,
                line_height,
                scroll_y,
                viewport_h,
                (viewport_w - margins.right).max(0.0),
            )?;
        renderer
            .last_scroll_placeholder_rows
            .set(placeholder_visible);
        breakdown.decoration_us = breakdown
            .decoration_us
            .saturating_add(elapsed_us(placeholder_started));

        let use_wrap_paint = params.view.soft_wrap;
        let body_start = Instant::now();
        let mut caret_drew_any = if use_wrap_paint {
            crate::wrap_paint::paint_display_lines(
                &renderer.d2d_context,
                cache,
                &renderer.dwrite_factory,
                rope,
                selections,
                params,
                margins,
                line_height,
                column_advance,
                tab_advance,
                scroll_y,
                viewport_h,
                crate::wrap_paint::WrapPaintBrushes {
                    bg: &bg_brush,
                    fg: &fg_brush,
                    text_roles: text_role_brushes,
                    caret: &caret_brush,
                    secondary_caret: &secondary_caret_brush,
                    selection: &selection_brush,
                    search_match: &search_match_brush,
                    search_match_active: &search_match_active_brush,
                    inline_highlight_bg: &inline_highlight_bg_brush,
                    formula_value: &formula_value_brush,
                    formula_error: &formula_error_brush,
                    inline_code_bg: &inline_code_bg_brush,
                },
                &renderer.last_inline_code_hits,
                &renderer.last_soft_wrap_overflow,
            )?
        } else {
            renderer.clear_unwrapped_frame_state();
            false
        };
        if !use_wrap_paint {
            caret_drew_any = crate::renderer_line_text_pass::paint_line_text_pass(
                &renderer.d2d_context,
                &renderer.dwrite_factory,
                cache,
                rope,
                selections,
                params,
                crate::renderer_line_text_pass::LineTextGeometry {
                    margins,
                    body_translate,
                    line_height,
                    column_advance,
                    scroll_y,
                    wrap_width_dip,
                    max_layout_width,
                    first_visible,
                    last_visible,
                },
                crate::renderer_line_text_pass::LineTextBrushes {
                    fg: &fg_brush,
                    text_roles: text_role_brushes,
                    bg: &bg_brush,
                    caret: &caret_brush,
                    secondary_caret: &secondary_caret_brush,
                    selection: &selection_brush,
                    search_match: &search_match_brush,
                    search_match_active: &search_match_active_brush,
                    inline_highlight_bg: &inline_highlight_bg_brush,
                    formula_value: &formula_value_brush,
                    formula_error: &formula_error_brush,
                    inline_code_bg: &inline_code_bg_brush,
                },
            )?;
        }
        renderer
            .last_body_paint_us
            .set(body_start.elapsed().as_micros() as u64);

        // Table-chrome replay timing is reported on its own bucket
        // via `last_table_chrome_stats`; do not also add to
        // `decoration_us` to avoid double-counting.
        crate::renderer_table_chrome::run_replay(
            renderer,
            &mut table_chrome_plan,
            params.body_origin,
            margins.left,
            line_height,
            scroll_y,
        )?;
        // Active-cell outline: drawn AFTER chrome replay so it sits on
        // top of the cached chrome bitmap. The chrome cache cannot bake
        // the outline because caret position changes per-frame; this
        // pass repaints fresh each frame. The outline (and its
        // translucent selected-cell fill) uses the themeable
        // `markdown.table.active_cell_outline` brush; the in-cell caret
        // bar keeps the editor caret color so it matches the body caret.
        crate::renderer_table_chrome::paint_focused_active_cell_outlines(
            &renderer.d2d_context,
            &renderer.dwrite_factory,
            params.format,
            params.table_layouts,
            params.frame_display,
            rope,
            selections,
            params.body_origin,
            margins.left,
            line_height,
            scroll_y,
            column_advance,
            &table_active_cell_brush,
            &caret_brush,
        );

        let spell_started = Instant::now();
        crate::spell::paint_spell_spans(
            &renderer.d2d_context,
            &render_target,
            cache,
            params,
            margins.left,
            line_height,
            scroll_y,
            wrap_width_dip,
            first_visible,
            last_visible,
        )?;
        breakdown.decoration_us = breakdown
            .decoration_us
            .saturating_add(elapsed_us(spell_started));

        breakdown.horizontal_rules_us = paint_horizontal_rules_pass(
            &renderer.d2d_context,
            rope,
            selections,
            params,
            &hr_brush,
            line_height,
            margins.left,
            editor_w,
            scroll_y,
        );

        let focus_dim_started = Instant::now();
        crate::renderer_focus_dim_pass::paint_focus_dim_pass(
            &renderer.d2d_context,
            &render_target,
            rope,
            selections,
            params,
            margins,
            body_translate,
            line_height,
            scroll_y,
            editor_w,
            first_visible,
            last_visible,
        )?;
        breakdown.decoration_us = breakdown
            .decoration_us
            .saturating_add(elapsed_us(focus_dim_started));

        let post_text_timings = crate::chrome_post::paint_post_text_chrome(
            &renderer.d2d_context,
            &renderer.dwrite_factory,
            params.format,
            rope,
            selections,
            params,
            margins,
            body_translate,
            line_height,
            scroll_y,
            viewport_h,
            column_advance,
            tab_advance,
            first_visible,
            last_visible,
            crate::chrome_post::PostTextBrushes {
                indent_guide: &indent_guide_brush,
                indent_guide_active: &indent_guide_active_brush,
                line_number: &line_number_brush,
                line_number_active: &line_number_active_brush,
            },
        )?;
        breakdown.indent_guides_us = post_text_timings.indent_guides_us;
        breakdown.line_numbers_us = post_text_timings.line_numbers_us;

        renderer.d2d_context.PopAxisAlignedClip();

        breakdown.minimap_us = minimap_pass::paint_minimap_pass(
            renderer,
            rope,
            params,
            viewport_w,
            viewport_h,
            line_height,
            scroll_y,
        );

        if let Some(draw) = params.search_minimap {
            let started = Instant::now();
            crate::search_minimap_paint::paint_search_minimap(&renderer.d2d_context, draw)?;
            breakdown.search_ticks_us = elapsed_us(started);
        }

        let caret_fallback_started = Instant::now();
        if !caret_drew_any
            && params.view_options.caret_visible
            && (rope.len_bytes() == 0 || selections.is_empty())
        {
            let caret_rect = caret_rect_for_shape(
                margins.left,
                -scroll_y,
                line_height,
                column_advance,
                params.view_options.caret_shape,
                params.view_options.caret_bar_width_px,
            );
            renderer
                .d2d_context
                .FillRectangle(&caret_rect, &caret_brush);
        }
        breakdown.decoration_us = breakdown
            .decoration_us
            .saturating_add(elapsed_us(caret_fallback_started));

        let content_h =
            crate::scrollbar::content_height_for_scrollbar(params.frame_display, line_height);
        let post_body_start = Instant::now();
        let post_body_output = crate::renderer_post_body::paint_post_body(
            crate::renderer_post_body::PostBodyContext {
                device_context: &renderer.d2d_context,
                dwrite: &renderer.dwrite_factory,
                render_target: &render_target,
                layout_cache: cache,
                image_cache: &renderer.image_cache,
                image_hits: &renderer.last_image_hits,
                fg_brush: &fg_brush,
                bg_brush: &bg_brush,
                chrome_command_list: &renderer.chrome_command_list,
            },
            params,
            viewport_w,
            viewport_h,
            editor_w,
            line_height,
            margins.left,
            scroll_y,
            content_h,
        )?;
        renderer
            .last_post_body_paint_us
            .set(post_body_start.elapsed().as_micros() as u64);
        renderer.last_post_body_stages.set(post_body_output.stages);
        renderer
            .last_chrome_path_stats
            .set(post_body_output.chrome_path);

        // Pull the sub-stages painted inside `paint_post_body` that
        // map to dedicated chrome-overlay buckets, and roll the rest
        // into the catch-all so the breakdown sum matches
        // `post_body_paint_us`.
        let post = post_body_output.stages;
        breakdown.outline_sidebar_us = post.outline_us;
        breakdown.scrollbar_us = post.scrollbar_us;
        breakdown.code_copy_button_us = post_body_output.code_copy_button_us;
        let post_other_us = post
            .brush_setup_us
            .saturating_add(post.spectator_bodies_us)
            .saturating_add(post.motion_overlays_us)
            .saturating_add(post.pane_chrome_us)
            .saturating_add(post.status_bar_us)
            .saturating_add(post.inline_images_us)
            .saturating_add(post.hud_us)
            .saturating_add(post.modal_overlays_us);
        breakdown.decoration_us = breakdown.decoration_us.saturating_add(post_other_us);

        renderer.last_chrome_overlay_breakdown.set(breakdown);

        renderer.d2d_context.EndDraw(None, None)?;
    }

    Ok(())
}
