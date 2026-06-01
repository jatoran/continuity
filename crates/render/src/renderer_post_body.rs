//! Post-body chrome dispatch pass — extracted from
//! [`crate::Renderer::draw_buffer_no_present`] so the renderer file
//! stays under the conventions cap.
//!
//! Runs after the body text / decoration / focus-dim passes; resets the
//! D2D transform to identity (so chrome paints in screen-absolute
//! coords) and dispatches the non-focused pane bodies, per-pane tab
//! strips, status bar, outline sidebar, inline images, time-machine
//! HUD, and the modal overlay layer (palette / find / quick-open / …).
//!
//! Thread ownership: UI thread.

use std::cell::RefCell;
use std::time::Instant;

use continuity_layout::LayoutCache;
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1RenderTarget, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::IDWriteFactory;

use crate::chrome_command_list::ChromeCommandList;
use crate::image_cache::ImageCache;
use crate::inline_image_types::InlineImageHit;
use crate::overlay_motion::paint_overlay_with_motion;
use crate::params::{DrawParams, Rgba};
use crate::Error;
use crate::RendererPostBodyStages;

/// Inputs that don't fit naturally as positional parameters — bundled
/// so the call site at [`crate::Renderer::draw_buffer_no_present`] reads
/// cleanly.
pub(crate) struct PostBodyContext<'a> {
    pub device_context: &'a ID2D1DeviceContext,
    pub dwrite: &'a IDWriteFactory,
    pub render_target: &'a ID2D1RenderTarget,
    pub layout_cache: &'a mut LayoutCache,
    pub image_cache: &'a RefCell<ImageCache>,
    pub image_hits: &'a RefCell<Vec<InlineImageHit>>,
    pub fg_brush: &'a ID2D1SolidColorBrush,
    pub bg_brush: &'a ID2D1SolidColorBrush,
    /// Retained static-chrome command list replayed after pane chrome.
    pub chrome_command_list: &'a RefCell<ChromeCommandList>,
}

/// Output from the post-body pass.
pub(crate) struct PostBodyOutput {
    /// Post-body stage timings.
    pub stages: RendererPostBodyStages,
    /// Retained static-chrome record/replay stats.
    pub chrome_path: crate::ChromePathStats,
    /// Microseconds spent inside the fenced-code-block copy-button
    /// hover affordance. Reported separately from
    /// `stages.scrollbar_us` because the affordance can dominate
    /// chrome-overlay cost during scroll if its early-return gate is
    /// loose. Feeds `chrome_overlay_code_copy_button_us` on
    /// `event:renderer_draw_stages`.
    pub code_copy_button_us: u64,
}

/// Run every paint phase from "reset transform to identity" through the
/// modal overlay layer, ending just before the renderer's `EndDraw`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_post_body(
    ctx: PostBodyContext<'_>,
    params: &DrawParams<'_>,
    viewport_w: f32,
    viewport_h: f32,
    editor_w: f32,
    line_height: f32,
    margins_left: f32,
    scroll_y: f32,
    content_h: f32,
) -> Result<PostBodyOutput, Error> {
    let total_start = Instant::now();
    let mut stages = RendererPostBodyStages::default();
    let PostBodyContext {
        device_context,
        dwrite,
        render_target,
        layout_cache,
        image_cache,
        image_hits,
        fg_brush,
        bg_brush,
        chrome_command_list,
    } = ctx;

    // Reset to identity for chrome (tab strips, borders) + overlay so
    // they paint in screen-absolute coords.
    let identity = Matrix3x2 {
        M11: 1.0,
        M12: 0.0,
        M21: 0.0,
        M22: 1.0,
        M31: 0.0,
        M32: 0.0,
    };
    unsafe {
        device_context.SetTransform(&identity);
    }

    let mkb = |rgba: Rgba| -> Result<ID2D1SolidColorBrush, Error> {
        Ok(unsafe { render_target.CreateSolidColorBrush(&D2D1_COLOR_F::from(rgba), None)? })
    };
    let brush_start = Instant::now();
    let line_number_brush = mkb(params.colors.line_number)?;
    let line_number_active_brush = mkb(params.colors.line_number_active)?;
    let placeholder_brush = mkb(
        crate::renderer_scroll_placeholder::scroll_placeholder_color(
            params.colors.loading_overlay_bg,
        ),
    )?;
    // Spectator panes paint markdown extensions (inline color, pipe-
    // tables, formula overrides) the same way the focused pane does, so
    // they need the same theme-derived brushes.
    let inline_highlight_bg_brush = mkb(params.markdown_colors.inline_highlight_bg)?;
    let code_panel_brush = mkb(params.markdown_colors.code_block_bg)?;
    let code_panel_header = crate::decoration_paint::compute_code_block_header_color(
        params.markdown_colors.code_block_bg,
    );
    let code_panel_header_brush = mkb(code_panel_header)?;
    let blockquote_bar_brush = mkb(params.markdown_colors.blockquote_bar)?;
    let hr_brush = mkb(params.markdown_colors.hr)?;
    let inline_code_bg_brush = mkb(params.markdown_colors.code_bg)?;
    let text_role_brush_set = crate::text_role_effects::TextRoleBrushSet::new(
        render_target,
        &params.markdown_colors,
        params.colors.fg,
    )?;
    let text_role_brushes = text_role_brush_set.refs();
    let formula_value_brush = mkb(params.markdown_colors.formula_value)?;
    let formula_error_brush = mkb(params.markdown_colors.formula_error)?;
    let table_border_brush = mkb(params.markdown_colors.table_border)?;
    let table_header_bg_brush = mkb(params.markdown_colors.table_header_bg)?;
    let table_alignment_bg_brush = mkb(params.markdown_colors.table_alignment_bg)?;
    let table_active_cell_brush = mkb(params.markdown_colors.table_active_cell_outline)?;
    let outline_colors = params.outline.map(|data| data.colors).unwrap_or_default();
    let outline_bg_brush = mkb(outline_colors.bg)?;
    let outline_fg_brush = mkb(outline_colors.fg)?;
    let outline_fg_active_brush = mkb(outline_colors.fg_active)?;
    let outline_separator_brush = mkb(outline_colors.separator)?;
    let minimap_colors = crate::minimap::MinimapColors {
        bg: params.colors.minimap_bg,
        fg: params.colors.minimap_fg,
        viewport_indicator: params.colors.minimap_viewport_indicator,
    };
    stages.brush_setup_us = elapsed_us(brush_start);

    // Phase 16.5: non-focused pane bodies (split into crate::pane_body).
    let spectator_start = Instant::now();
    unsafe {
        crate::pane_body::paint_all_pane_bodies(
            device_context,
            dwrite,
            layout_cache,
            params,
            line_height,
            crate::pane_body::PaneBodyBrushes {
                fg: fg_brush,
                text_roles: text_role_brushes,
                bg: bg_brush,
                placeholder: &placeholder_brush,
                line_number: &line_number_brush,
                line_number_active: &line_number_active_brush,
                inline_highlight_bg: &inline_highlight_bg_brush,
                code_panel: &code_panel_brush,
                code_panel_header: &code_panel_header_brush,
                blockquote_bar: &blockquote_bar_brush,
                hr: &hr_brush,
                inline_code_bg: &inline_code_bg_brush,
                formula_value: &formula_value_brush,
                formula_error: &formula_error_brush,
                table_border: &table_border_brush,
                table_header_bg: &table_header_bg_brush,
                table_alignment_bg: &table_alignment_bg_brush,
                table_active_cell_outline: &table_active_cell_brush,
                outline_bg: &outline_bg_brush,
                outline_fg: &outline_fg_brush,
                outline_fg_active: &outline_fg_active_brush,
                outline_separator: &outline_separator_brush,
                minimap_colors,
            },
        )?;
    }
    stages.spectator_bodies_us = elapsed_us(spectator_start);

    let motion_start = Instant::now();
    if let Some(glow) = params.jump_glow {
        crate::jump_glow_paint::paint_jump_glow(
            device_context,
            glow,
            params.body_origin,
            viewport_w,
            viewport_h,
            line_height,
            scroll_y,
        )?;
    }

    if let Some(pulse) = params.edit_pulse {
        crate::edit_pulse_paint::paint_edit_pulse(
            device_context,
            pulse,
            params.body_origin,
            viewport_w,
            viewport_h,
            line_height,
            scroll_y,
        )?;
    }
    stages.motion_overlays_us = elapsed_us(motion_start);

    let pane_chrome_start = Instant::now();
    crate::pane_chrome::dispatch_pane_chrome(device_context, dwrite, params)?;
    stages.pane_chrome_us = elapsed_us(pane_chrome_start);

    let chrome_path = chrome_command_list.borrow().replay(device_context)?;

    // Phase C1: paint status-bar segments supplied by the UI layer.
    let status_bar_start = Instant::now();
    if params.view_options.show_status_bar {
        if let Some(data) = params.status_bar {
            // The status bar is chrome, not content — it must not grow
            // with Ctrl+wheel body zoom (the bar height is fixed, so a
            // zoomed font clips). `base_font_size_dip` already carries the
            // body zoom, so divide it back out to recover the unscaled
            // base size and pin the bar to that.
            let status_bar_font_size =
                params.base_font_size_dip / params.view.font_size_scale.max(0.01);
            crate::status_bar::paint_status_bar_frame_text(
                device_context,
                dwrite,
                params.format,
                data,
                viewport_w,
                params.client_height_dip,
                status_bar_font_size,
                &mkb,
            );
        }
    }
    stages.status_bar_us = elapsed_us(status_bar_start);

    if let Some(file_tree) = params.file_tree {
        crate::file_tree_paint::paint_file_tree(device_context, dwrite, file_tree, params.format)?;
    }

    // Phase F2: paint the right-docked outline sidebar.
    let outline_start = Instant::now();
    crate::outline_paint::dispatch_outline_text_paint(
        device_context,
        dwrite,
        params,
        viewport_w,
        viewport_h,
        &mkb,
    );
    stages.outline_us = elapsed_us(outline_start);

    // F5 — inline image paint. Runs after text / decoration / outline so
    // images sit *under* overlays (palette, find bar) but *above* the
    // body fill. Hits go into the renderer's `last_image_hits` ring so
    // the UI mouse handler can route clicks on collapsed affordances.
    let inline_images_start = Instant::now();
    image_hits.borrow_mut().clear();
    if let Some(images) = params.images {
        if !images.is_empty() {
            // Visible bottom of the body in pane-body-relative coords.
            // Clamps an expanded image so its bitmap can't overflow
            // past the pane body or paint over the status bar (which
            // is painted earlier in this pass but otherwise would be
            // overdrawn by a tall expanded image).
            let status_bar_reserve = if params.view_options.show_status_bar {
                let bar_top_abs =
                    (params.client_height_dip - crate::STATUS_BAR_HEIGHT_DIP).max(0.0);
                (bar_top_abs - params.body_origin.1).max(0.0)
            } else {
                viewport_h
            };
            let max_image_bottom = viewport_h.min(status_bar_reserve).max(0.0);
            let mut cache = image_cache.borrow_mut();
            let mut hits = image_hits.borrow_mut();
            crate::image_paint::paint_inline_images(
                device_context,
                &mut cache,
                dwrite,
                params.format,
                images,
                params.body_origin,
                margins_left,
                scroll_y,
                editor_w,
                line_height,
                max_image_bottom,
                params.colors.line_number,
                &mut hits,
            );
        }
    }

    // Spectator inline-image paint. Same painter as the focused pass
    // above, fed each pane's own rect / view / placements. Hit-test
    // results go into a throwaway Vec — chevron clicks on a spectator
    // require focusing the pane first (clicking into it), at which
    // point its placements move through the focused-pane image_hits
    // path. This avoids ambiguous routing when the same screen point
    // lies inside multiple panes' (image, hit) rings.
    {
        let mut cache = image_cache.borrow_mut();
        let mut throwaway_hits: Vec<InlineImageHit> = Vec::new();
        for body in params.pane_bodies {
            if body.images.is_empty() {
                continue;
            }
            let (bx, by, bw, bh) = body.rect;
            let margins_left_spec = if params.view_options.line_numbers {
                crate::chrome::gutter_width_for_line_count(
                    params.base_font_size_dip,
                    body.rope.len_lines(),
                ) + crate::chrome::GUTTER_BODY_GAP_DIP
            } else {
                crate::chrome::BODY_LEFT_PADDING_DIP
            };
            let body_inner_w =
                (bw - margins_left_spec - crate::chrome::BODY_RIGHT_PADDING_DIP).max(0.0);
            crate::image_paint::paint_inline_images(
                device_context,
                &mut cache,
                dwrite,
                params.format,
                body.images,
                (bx, by),
                margins_left_spec,
                body.view.scroll_y_dip,
                body_inner_w,
                line_height,
                bh.max(0.0),
                params.colors.line_number,
                &mut throwaway_hits,
            );
            throwaway_hits.clear();
        }
    }
    stages.inline_images_us = elapsed_us(inline_images_start);

    // Phase I1: time-machine slider HUD. Painted after pane chrome (so
    // it floats above the buffer body) but before modal overlays
    // (palette / find / quick-open / goto) because those should occlude
    // the slider when both are up. UI sets `time_machine_hud` only when
    // `time_machine.timeline_visible` is true.
    let hud_start = Instant::now();
    if let Some(hud) = params.time_machine_hud {
        crate::time_machine_hud_paint::paint_time_machine_hud(device_context, dwrite, hud)?;
    }
    stages.hud_us = elapsed_us(hud_start);

    // Minimal y-scrollbar: a thin thumb at the right edge of the
    // text-body column when content exceeds the viewport. Painted
    // after pane chrome / outline / images so it sits on top of the
    // body, but before overlays so palette / find / quick-open
    // occlude it.
    let scrollbar_start = Instant::now();
    unsafe {
        crate::scrollbar::paint_scrollbar(
            device_context,
            params.body_origin.0 + margins_left + editor_w,
            params.body_origin.1,
            scroll_y,
            viewport_h,
            content_h,
            &line_number_brush,
        );
    }
    stages.scrollbar_us = elapsed_us(scrollbar_start);

    // Fenced-code-block copy-button hover overlay. Painted after the
    // body chrome (scrollbar / line-number gutter) so the button
    // can sit over the gutter when a fenced block runs to the
    // viewport's left edge, and before the loading / modal overlays
    // so any of those still occlude the button when appropriate.
    let code_copy_button_start = Instant::now();
    if let Some(button) = params.code_copy_button.as_ref() {
        crate::code_copy_button_paint::paint_code_copy_button(
            device_context,
            params.format,
            button,
            &params.markdown_colors,
            params.colors.caret,
        )?;
    }
    let code_copy_button_us = elapsed_us(code_copy_button_start);

    // P0.8.3: paint the transient "building view" overlay after the
    // scrollbar so it sits over the body and pane chrome, and before
    // modal overlays so palette/find/quick-open occlude it.
    if let Some(loading) = params.loading_overlay {
        let motion = params.loading_overlay_motion.unwrap_or_default();
        crate::loading_overlay::paint_loading_overlay(
            device_context,
            dwrite,
            params.format,
            loading,
            params.body_origin,
            motion,
        )?;
    }

    let modal_start = Instant::now();
    if let Some(overlay) = params.overlay {
        paint_overlay_with_motion(
            device_context,
            dwrite,
            params.format,
            overlay,
            params.overlay_motion,
        )?;
    }

    if let Some(hud) = params.chord_hud {
        paint_overlay_with_motion(
            device_context,
            dwrite,
            params.format,
            hud,
            params.chord_hud_motion,
        )?;
    }
    stages.modal_overlays_us = elapsed_us(modal_start);

    stages.total_us = elapsed_us(total_start);
    Ok(PostBodyOutput {
        stages,
        chrome_path,
        code_copy_button_us,
    })
}

fn elapsed_us(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}
