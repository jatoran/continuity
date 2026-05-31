//! Per-line text paint loop — extracted from
//! [`crate::Renderer::draw_buffer_no_present`] so the renderer file
//! stays under the conventions cap.
//!
//! Iterates `first_visible..last_visible` source lines, ensures each
//! line's cached [`continuity_layout`] entry, installs a per-line
//! transform, paints inline-highlight backgrounds → selection band →
//! the text → inline-color post-text overlays → table-formula
//! overrides → carets, and restores the body-level transform.
//!
//! Used only when `params.view.soft_wrap == false`; the soft-wrap path
//! is handled by [`crate::wrap_paint::paint_display_lines`] at the call
//! site.
//!
//! Thread ownership: UI thread.

use continuity_layout::LayoutCache;
use ropey::Rope;
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Graphics::Direct2D::Common::D2D_POINT_2F;
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1SolidColorBrush, D2D1_DRAW_TEXT_OPTIONS_NONE,
};
use windows::Win32::Graphics::DirectWrite::IDWriteFactory;

use crate::chrome::ContentMargins;
use crate::chrome_caret::caret_rect_for_shape;
use crate::params::DrawParams;
use crate::text_helpers::{
    apply_footnote_drawing_effects, build_key_for_spec, caret_utf16_for_line,
    draw_selection_line_with_layout, ensure_line_layout_for_spec, hit_test_x,
};
use crate::Error;

/// Brushes consumed by the per-line text pass.
pub(crate) struct LineTextBrushes<'a> {
    pub fg: &'a ID2D1SolidColorBrush,
    pub footnote: &'a ID2D1SolidColorBrush,
    pub bg: &'a ID2D1SolidColorBrush,
    pub caret: &'a ID2D1SolidColorBrush,
    pub secondary_caret: &'a ID2D1SolidColorBrush,
    pub selection: &'a ID2D1SolidColorBrush,
    pub search_match: &'a ID2D1SolidColorBrush,
    pub search_match_active: &'a ID2D1SolidColorBrush,
    pub inline_highlight_bg: &'a ID2D1SolidColorBrush,
    pub formula_value: &'a ID2D1SolidColorBrush,
    pub formula_error: &'a ID2D1SolidColorBrush,
    /// Inline `` `code` `` background fill.
    pub inline_code_bg: &'a ID2D1SolidColorBrush,
}

/// Scalar layout / geometry inputs consumed by the per-line text pass.
pub(crate) struct LineTextGeometry {
    pub margins: ContentMargins,
    pub body_translate: Matrix3x2,
    pub line_height: f32,
    pub column_advance: f32,
    pub scroll_y: f32,
    pub wrap_width_dip: u32,
    pub max_layout_width: f32,
    pub first_visible: usize,
    pub last_visible: usize,
}

/// Run the non-wrap per-line text paint loop. Returns `true` when at
/// least one caret rect was painted (so the caller can skip its empty-
/// buffer / out-of-range caret fallback).
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_line_text_pass(
    device_context: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    cache: &mut LayoutCache,
    rope: &Rope,
    selections: &[continuity_text::Selection],
    params: &DrawParams<'_>,
    geom: LineTextGeometry,
    brushes: LineTextBrushes<'_>,
) -> Result<bool, Error> {
    let LineTextGeometry {
        margins,
        body_translate,
        line_height,
        column_advance,
        scroll_y,
        wrap_width_dip,
        max_layout_width,
        first_visible,
        last_visible,
    } = geom;
    let mut caret_drew_any = false;
    for line_idx in first_visible..last_visible {
        // Folded source lines have no DisplayLineSpec — skip them
        // entirely. Plain text always produces a spec.
        let Some(spec) = params.frame_display.line(line_idx) else {
            continue;
        };
        let y = line_idx as f32 * line_height - scroll_y;
        let key = build_key_for_spec(spec, params.document, params.font_state, wrap_width_dip);
        ensure_line_layout_for_spec(
            cache,
            dwrite,
            spec,
            key,
            params.format,
            max_layout_width,
            params.base_font_size_dip,
            params.heading_scale,
        )?;
        let entry = cache.get(&key).expect("just inserted");
        // Phase 17.6: per-line SetTransform → layout-local coords.
        let line_translate = Matrix3x2 {
            M11: 1.0,
            M12: 0.0,
            M21: 0.0,
            M22: 1.0,
            M31: params.body_origin.0 + margins.left,
            M32: params.body_origin.1 + y,
        };
        unsafe {
            device_context.SetTransform(&line_translate);
        }
        crate::markdown_extension_paint::paint_inline_color_pre_text(
            device_context,
            entry.layout,
            entry.text,
            rope,
            selections,
            params,
            line_idx,
            line_height,
            brushes.inline_highlight_bg,
        );

        if let Some(decorations) = params.decorations {
            let line_start_byte = rope.line_to_byte(line_idx);
            let line_end_byte = line_start_byte + rope.line(line_idx).len_bytes();
            let caret_bytes_for_line =
                crate::inline_color_paint::caret_bytes_from_selections(rope, selections);
            // No-wrap mode does not publish hits — the inline copy
            // button rides only on the soft-wrap path for now. The
            // BG fill is still painted so the distinctness audit
            // result holds regardless of wrap mode.
            let body_x_in_client = params.body_origin.0 + margins.left;
            let body_y_in_client = params.body_origin.1;
            crate::inline_code_paint::paint_inline_code_backgrounds_line(
                device_context,
                entry.layout,
                entry.text,
                params.frame_display,
                line_idx,
                line_start_byte..line_end_byte,
                line_height,
                &decorations.inlines,
                &caret_bytes_for_line,
                brushes.inline_code_bg,
                body_x_in_client,
                body_y_in_client,
                y,
                None,
            );
        }
        draw_selection_line_with_layout(
            device_context,
            rope,
            entry.text,
            entry.layout,
            line_idx,
            line_height,
            selections,
            brushes.selection,
            params.frame_display,
        );
        if let Some(search) = params.search_minimap {
            let search_paint = crate::search_highlight_paint::SearchHighlightPaint::new(
                device_context,
                entry.layout,
                line_height,
                &search.body_highlights,
                brushes.search_match,
                brushes.search_match_active,
            );
            crate::search_highlight_paint::paint_search_highlights_line(
                &search_paint,
                entry.text,
                params.frame_display,
                line_idx,
            );
        }
        apply_footnote_drawing_effects(entry.layout, spec.style_runs(), brushes.footnote);
        unsafe {
            device_context.DrawTextLayout(
                D2D_POINT_2F { x: 0.0, y: 0.0 },
                entry.layout,
                brushes.fg,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }
        crate::markdown_extension_paint::paint_inline_color_post_text(
            device_context,
            dwrite,
            entry.layout,
            entry.text,
            rope,
            selections,
            params,
            line_idx,
            line_height,
        );
        crate::markdown_extension_paint::paint_table_overrides_post_text(
            device_context,
            dwrite,
            entry.layout,
            entry.text,
            rope,
            params,
            line_idx,
            line_height,
            crate::table_formula_paint::TableFormulaBrushes {
                bg: brushes.bg,
                value: brushes.formula_value,
                error: brushes.formula_error,
            },
        );
        // P14.1 — visual-table chrome moved to `renderer_table_chrome`;
        // see `wrap_paint` for the matching comment.
        if params.view_options.caret_visible {
            for (idx, selection) in selections.iter().enumerate() {
                if selection.head.line as usize == line_idx {
                    let utf16 = caret_utf16_for_line(
                        entry.text,
                        params.frame_display,
                        line_idx,
                        selection.head.byte_in_line as usize,
                    );
                    let caret_x = hit_test_x(entry.layout, utf16).unwrap_or(0.0);
                    let caret_rect = caret_rect_for_shape(
                        caret_x,
                        0.0,
                        line_height,
                        column_advance,
                        params.view_options.caret_shape,
                        params.view_options.caret_bar_width_px,
                    );
                    let brush = if idx == 0 {
                        brushes.caret
                    } else {
                        brushes.secondary_caret
                    };
                    unsafe {
                        device_context.FillRectangle(&caret_rect, brush);
                    }
                    caret_drew_any = true;
                }
            }
        }
    }
    // Restore the body-level transform once the per-line block has
    // finished so chrome painters that follow paint in body-relative
    // coords (not layout-local).
    unsafe {
        device_context.SetTransform(&body_translate);
    }
    Ok(caret_drew_any)
}
