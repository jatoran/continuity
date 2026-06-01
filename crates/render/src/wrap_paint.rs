//! Phase 17.6 soft-wrap paint path: iterates *display* lines (one
//! `IDWriteTextLayout` per visual row) and paints each at its own
//! `(x, y)` so wrap continuations hang-indent under the source line's
//! leading whitespace.
//!
//! Activated by [`crate::Renderer::draw_buffer`] when both
//! `params.frame_display` is `Some` and `params.view.soft_wrap` is on.
//! The legacy DirectWrite-driven wrap path (per-source-line layouts
//! with intra-layout wrap) stays as the fallback.
//!
//! **Thread ownership**: UI thread (D2D / DirectWrite handles).

use continuity_display_map::{DisplayByte, DisplayLineSpec, SourceByte};
use continuity_layout::LayoutCache;
use continuity_text::{Selection, SelectionKind};
use ropey::Rope;
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Graphics::Direct2D::Common::{D2D_POINT_2F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1SolidColorBrush, D2D1_DRAW_TEXT_OPTIONS_NONE,
};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory, IDWriteTextLayout, DWRITE_TEXT_METRICS,
};

use crate::chrome::ContentMargins;
use crate::chrome_caret::caret_rect_for_shape;
use crate::display_projection::FrameDisplay;
use crate::inline_color_paint::caret_bytes_from_selections;
use crate::params::DrawParams;
use crate::table_formula_paint::TableFormulaBrushes;
use crate::text_helpers::{build_key_for_spec, ensure_line_layout_for_spec, hit_test_x};
use crate::text_role_effects::{apply_role_drawing_effects, TextRoleBrushes};
use crate::Error;

/// Bundle of theme-derived brushes used by [`paint_display_lines`].
pub(crate) struct WrapPaintBrushes<'a> {
    /// Editor background brush.
    pub bg: &'a ID2D1SolidColorBrush,
    /// Foreground glyph brush.
    pub fg: &'a ID2D1SolidColorBrush,
    /// Styled text-role foreground brushes.
    pub text_roles: TextRoleBrushes<'a>,
    /// Primary caret brush.
    pub caret: &'a ID2D1SolidColorBrush,
    /// Secondary-caret brush (multi-cursor overlay).
    pub secondary_caret: &'a ID2D1SolidColorBrush,
    /// Selection-rect brush.
    pub selection: &'a ID2D1SolidColorBrush,
    /// Search-match background brush.
    pub search_match: &'a ID2D1SolidColorBrush,
    /// Active search-match background brush.
    pub search_match_active: &'a ID2D1SolidColorBrush,
    /// Inline-highlight background brush.
    pub inline_highlight_bg: &'a ID2D1SolidColorBrush,
    /// Formula value foreground brush.
    pub formula_value: &'a ID2D1SolidColorBrush,
    /// Formula error foreground brush.
    pub formula_error: &'a ID2D1SolidColorBrush,
    /// Inline `` `code` `` background fill brush.
    pub inline_code_bg: &'a ID2D1SolidColorBrush,
}

/// Paint every visible display line. Each line installs its own
/// per-line `SetTransform` (Phase 17.6 cleanup) so the paint code
/// operates at layout-local `(0, 0)`; the renderer restores the
/// body-level transform on return. Returns `true` if at least one
/// caret was drawn.
///
/// `inline_code_hits` collects one entry per painted inline-code
/// span in client DIPs so the UI mouse handler can drive the inline
/// copy-button hover affordance. The vec is cleared on entry.
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn paint_display_lines(
    ctx: &ID2D1DeviceContext,
    cache: &mut LayoutCache,
    factory: &IDWriteFactory,
    rope: &Rope,
    selections: &[Selection],
    params: &DrawParams<'_>,
    margins: ContentMargins,
    line_height: f32,
    column_advance: f32,
    tab_advance: f32,
    scroll_y: f32,
    viewport_h: f32,
    brushes: WrapPaintBrushes<'_>,
    inline_code_hits: &std::cell::RefCell<Vec<crate::InlineCodeHit>>,
    overflow_out: &std::cell::Cell<crate::SoftWrapOverflowSample>,
) -> Result<bool, Error> {
    inline_code_hits.borrow_mut().clear();
    // Diagnostic soft-wrap overflow detector (cheap: one `GetMetrics` per
    // visible row off an already-laid-out layout). A row overflows when
    // its painted visible advance runs past the text column's right edge
    // even though soft-wrap decided it fit. `content_right` and `paint_x`
    // are both body-origin-relative DIPs, so they compare directly.
    let content_right = (params.view.viewport_width_dip - margins.right).max(0.0);
    let mut overflow_sample = crate::SoftWrapOverflowSample::default();
    let fd = params.frame_display;
    let total_display = fd.display_line_count() as i64;
    let first_visible = ((scroll_y / line_height).floor() as i64).max(0) as u32;
    let last_visible = ((((scroll_y + viewport_h) / line_height).ceil() as i64) + 1)
        .clamp(0, total_display) as u32;
    let max_layout_width = f32::INFINITY;
    let mut caret_drew_any = false;
    let caret_bytes = caret_bytes_from_selections(rope, selections);

    for dl_idx in first_visible..last_visible {
        let Some(spec) = fd.display_line_by_index(dl_idx) else {
            continue;
        };
        let source_line = spec.source_line.raw() as usize;
        let leading_dip = if spec.is_wrap_continuation {
            // Measure the rendered advance of the leading whitespace
            // rather than its byte count: tabs render at DirectWrite's
            // resolved tab-stop width (~`indent_size` columns), not one
            // column each. Using bytes here makes tab-indented lines
            // wrap with a single-space-wide hanging indent per tab,
            // which is the "Tab only adds a space after wrap" bug.
            FrameDisplay::leading_whitespace_advance_dip(
                rope,
                source_line,
                column_advance,
                tab_advance,
            )
        } else {
            0.0
        };
        let paint_x = margins.left + leading_dip;
        let y = dl_idx as f32 * line_height - scroll_y;

        let key = build_key_for_spec(spec, params.document, params.font_state, 0);
        ensure_line_layout_for_spec(
            cache,
            factory,
            spec,
            key,
            params.format,
            max_layout_width,
            params.base_font_size_dip,
            params.heading_scale,
        )?;
        let Some(entry) = cache.get(&key) else {
            continue;
        };

        let line_translate = Matrix3x2 {
            M11: 1.0,
            M12: 0.0,
            M21: 0.0,
            M22: 1.0,
            M31: params.body_origin.0 + paint_x,
            M32: params.body_origin.1 + y,
        };
        ctx.SetTransform(&line_translate);

        crate::inline_color_paint::paint_inline_color_backgrounds_spec(
            ctx,
            entry.layout,
            entry.text,
            spec,
            line_height,
            params.inline_color_spans,
            &caret_bytes,
            brushes.inline_highlight_bg,
        );

        if let Some(decorations) = params.decorations {
            let body_x_in_client = params.body_origin.0 + paint_x;
            let body_y_in_client = params.body_origin.1;
            crate::inline_code_paint::paint_inline_code_backgrounds_spec(
                ctx,
                entry.layout,
                entry.text,
                spec,
                line_height,
                &decorations.inlines,
                &caret_bytes,
                brushes.inline_code_bg,
                body_x_in_client,
                body_y_in_client,
                y,
                Some(&mut *inline_code_hits.borrow_mut()),
            );
        }

        paint_selection_for_spec(
            ctx,
            rope,
            spec,
            entry.layout,
            line_height,
            selections,
            brushes.selection,
        );

        if let Some(search) = params.search_minimap {
            let search_paint = crate::search_highlight_paint::SearchHighlightPaint::new(
                ctx,
                entry.layout,
                line_height,
                &search.body_highlights,
                brushes.search_match,
                brushes.search_match_active,
            );
            crate::search_highlight_paint::paint_search_highlights_spec(
                &search_paint,
                entry.text,
                spec,
            );
        }

        apply_role_drawing_effects(entry.layout, spec.style_runs(), &brushes.text_roles);

        // Measure the fully-styled layout (heading scale + bold already
        // applied) against the text column. `metrics.width` is the visible
        // advance excluding trailing whitespace — what the user actually
        // sees, and what should never exceed the column the soft-wrap pass
        // wrapped against.
        if content_right > 0.0 {
            let mut metrics = DWRITE_TEXT_METRICS::default();
            if entry.layout.GetMetrics(&mut metrics).is_ok() {
                let row_right = paint_x + metrics.width;
                let overflow = row_right - content_right;
                if overflow > SOFT_WRAP_OVERFLOW_EPSILON_DIP {
                    overflow_sample.rows = overflow_sample.rows.saturating_add(1);
                    if overflow > overflow_sample.worst_overflow_dip {
                        overflow_sample.worst_overflow_dip = overflow;
                        overflow_sample.worst_source_line = source_line as u32;
                        overflow_sample.worst_display_row = dl_idx;
                        overflow_sample.worst_advance_dip = metrics.width;
                        overflow_sample.worst_wrap_width_dip =
                            (content_right - margins.left).max(0.0);
                        overflow_sample.worst_is_continuation = spec.is_wrap_continuation;
                        overflow_sample.worst_leading_dip = leading_dip;
                    }
                }
            }
        }

        ctx.DrawTextLayout(
            D2D_POINT_2F { x: 0.0, y: 0.0 },
            entry.layout,
            brushes.fg,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
        );

        crate::inline_color_paint::paint_inline_color_foregrounds_spec(
            ctx,
            factory,
            params.format,
            entry.layout,
            entry.text,
            spec,
            line_height,
            params.inline_color_spans,
            &caret_bytes,
        );

        crate::table_formula_paint::paint_table_overrides_spec(
            ctx,
            factory,
            params.format,
            entry.layout,
            entry.text,
            spec,
            line_height,
            params.table_overrides,
            params.table_layouts,
            &TableFormulaBrushes {
                bg: brushes.bg,
                value: brushes.formula_value,
                error: brushes.formula_error,
            },
        );

        // P14.1 — visual-table chrome is replayed once per table from
        // the renderer's `renderer_table_chrome` pass after the per-
        // line body loop, so the per-line painter doesn't repeat the
        // record cost.

        if params.view_options.caret_visible {
            for (idx, selection) in selections.iter().enumerate() {
                if selection.head.line as usize != source_line {
                    continue;
                }
                let head_byte_in_line = selection.head.byte_in_line as usize;
                let line_start_abs = rope.line_to_byte(source_line);
                let head_abs = line_start_abs + head_byte_in_line;
                let spec_start = spec.source_byte_start.raw() as usize;
                let spec_end = spec.source_byte_end.raw() as usize;
                // Caret belongs on this display line if its absolute
                // source byte falls inside the spec's range — with a
                // subtlety: a caret exactly at a wrap break point belongs
                // on the *later* display line (the one that starts there).
                let on_this_line = if spec.is_wrap_continuation {
                    head_abs >= spec_start && head_abs <= spec_end
                } else {
                    head_abs >= spec_start && head_abs < spec_end.max(spec_start + 1)
                        || head_abs == spec_end && head_abs == line_end_abs(rope, source_line)
                };
                if !on_this_line {
                    continue;
                }
                let display_byte = spec
                    .source_to_display(SourceByte::from_usize(head_abs))
                    .unwrap_or(DisplayByte(0));
                let utf16 = spec
                    .display_byte_to_utf16(display_byte)
                    .map(|u| u.raw() as usize)
                    .unwrap_or(0);
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
                ctx.FillRectangle(&caret_rect, brush);
                caret_drew_any = true;
            }
        }
    }

    overflow_out.set(overflow_sample);

    // Restore the body-level transform so callers can continue painting
    // in body-relative coords.
    let body_translate = Matrix3x2 {
        M11: 1.0,
        M12: 0.0,
        M21: 0.0,
        M22: 1.0,
        M31: params.body_origin.0,
        M32: params.body_origin.1,
    };
    ctx.SetTransform(&body_translate);

    Ok(caret_drew_any)
}

/// Below this many DIPs an apparent overflow is float / sub-pixel noise
/// (half a device pixel at 96 DPI) and is ignored by the detector.
const SOFT_WRAP_OVERFLOW_EPSILON_DIP: f32 = 0.5;

fn line_end_abs(rope: &Rope, source_line: usize) -> usize {
    let next = if source_line + 1 < rope.len_lines() {
        rope.line_to_byte(source_line + 1)
    } else {
        rope.len_bytes()
    };
    let start = rope.line_to_byte(source_line);
    let mut end = next;
    let slice = rope.byte_slice(start..next).to_string();
    if slice.ends_with('\n') {
        end -= 1;
        if slice.ends_with("\r\n") {
            end -= 1;
        }
    }
    end
}

#[allow(clippy::too_many_arguments)]
fn paint_selection_for_spec(
    ctx: &ID2D1DeviceContext,
    rope: &Rope,
    spec: &DisplayLineSpec,
    layout: &IDWriteTextLayout,
    line_height: f32,
    selections: &[Selection],
    brush: &ID2D1SolidColorBrush,
) {
    let spec_start = spec.source_byte_start.raw() as usize;
    let spec_end = spec.source_byte_end.raw() as usize;
    for selection in selections {
        if selection.is_collapsed() {
            continue;
        }
        let (abs_s, abs_e) = if selection.kind == SelectionKind::BlockWise {
            let source_line = spec.source_line.raw() as usize;
            let (start_line, end_line) = (
                selection.anchor.line.min(selection.head.line) as usize,
                selection.anchor.line.max(selection.head.line) as usize,
            );
            if source_line < start_line || source_line > end_line {
                continue;
            }
            let line_start = rope.line_to_byte(source_line);
            let s = selection
                .anchor
                .byte_in_line
                .min(selection.head.byte_in_line) as usize;
            let e = selection
                .anchor
                .byte_in_line
                .max(selection.head.byte_in_line) as usize;
            (line_start + s, line_start + e)
        } else {
            let range = selection.ordered_range();
            let (Ok(s), Ok(e)) = (
                range.start.to_byte_offset(rope),
                range.end.to_byte_offset(rope),
            ) else {
                continue;
            };
            (s, e)
        };
        let cs = abs_s.max(spec_start).min(spec_end);
        let ce = abs_e.max(spec_start).min(spec_end);
        if cs >= ce {
            continue;
        }
        let ds = spec
            .source_to_display(SourceByte::from_usize(cs))
            .unwrap_or(DisplayByte(0));
        let de = spec
            .source_to_display(SourceByte::from_usize(ce))
            .unwrap_or(DisplayByte(spec.display_len()));
        let utf16_s = spec.display_byte_to_utf16(ds).map(|u| u.raw()).unwrap_or(0);
        let utf16_e = spec
            .display_byte_to_utf16(de)
            .map(|u| u.raw())
            .unwrap_or(spec.display_len());
        let left = hit_test_x(layout, utf16_s as usize).unwrap_or(0.0);
        let right = hit_test_x(layout, utf16_e as usize).unwrap_or(left);
        let rect = D2D_RECT_F {
            left: left.min(right),
            top: 0.0,
            right: right.max(left).max(left + 1.0),
            bottom: line_height,
        };
        unsafe {
            ctx.FillRectangle(&rect, brush);
        }
    }
}
