//! Phase 17.6: post-text-paint chrome helpers — indent guides,
//! whitespace markers, line-number gutter. Retained static chrome
//! records ruler columns before this pass. Lifted out of
//! [`crate::renderer::Renderer::draw_buffer`] so that file stays under
//! the 600-line conventions cap once the wrap-paint dispatch is wired in.
//!
//! **Thread ownership**: UI thread.

use std::time::Instant;

use continuity_text::Selection;
use ropey::Rope;
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Graphics::Direct2D::{ID2D1DeviceContext, ID2D1SolidColorBrush};
use windows::Win32::Graphics::DirectWrite::{IDWriteFactory, IDWriteTextFormat};

use crate::chrome::{paint_whitespace_markers, ContentMargins};
use crate::chrome_fold::{compute_fold_headers, paint_fold_triangles};
use crate::chrome_indent_guides::paint_indent_guides;
use crate::chrome_line_numbers::paint_line_number_gutter;
use crate::params::DrawParams;
use crate::Error;

/// Bundle of chrome brushes used by [`paint_post_text_chrome`].
pub(crate) struct PostTextBrushes<'a> {
    /// Indent-guide rules.
    pub indent_guide: &'a ID2D1SolidColorBrush,
    /// Emphasized indent-guide rule for the caret's indent column.
    pub indent_guide_active: &'a ID2D1SolidColorBrush,
    /// Default gutter foreground.
    pub line_number: &'a ID2D1SolidColorBrush,
    /// Active-line gutter foreground.
    pub line_number_active: &'a ID2D1SolidColorBrush,
}

/// Sub-stage timings returned by [`paint_post_text_chrome`]. Feeds the
/// `chrome_overlay_indent_guides_us` / `chrome_overlay_line_numbers_us`
/// fields on `event:renderer_draw_stages`. Whitespace markers ride
/// alongside indent guides; fold triangles ride alongside line numbers
/// because both pairs share a transform and the user perceives them
/// as one chrome surface.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct PostTextChromeTimings {
    /// Indent guides + whitespace markers.
    pub indent_guides_us: u64,
    /// Line-number gutter + fold triangles + fold-header compute.
    pub line_numbers_us: u64,
}

/// Paint indent-guides, whitespace markers, and the
/// line-number gutter (in that z-order). Body-relative painters work
/// in *body-content* space — origin sits at `margins.left` — installed
/// here once instead of every painter computing `+ margins.left`. The
/// gutter painter runs at body-translate (absolute x from 0) so the
/// numbers land in the gutter to the left of the body.
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn paint_post_text_chrome(
    ctx: &ID2D1DeviceContext,
    factory: &IDWriteFactory,
    format: &IDWriteTextFormat,
    rope: &Rope,
    selections: &[Selection],
    params: &DrawParams<'_>,
    margins: ContentMargins,
    body_translate: Matrix3x2,
    line_height: f32,
    scroll_y: f32,
    viewport_h: f32,
    column_advance: f32,
    tab_advance: f32,
    first_visible: usize,
    last_visible: usize,
    brushes: PostTextBrushes<'_>,
) -> Result<PostTextChromeTimings, Error> {
    let mut timings = PostTextChromeTimings::default();
    // Body-content translate: body origin shifted right by margins.left.
    // Body-relative painters live here.
    let body_content_translate = Matrix3x2 {
        M11: 1.0,
        M12: 0.0,
        M21: 0.0,
        M22: 1.0,
        M31: body_translate.M31 + margins.left,
        M32: body_translate.M32,
    };
    let zero_left = ContentMargins {
        left: 0.0,
        right: margins.right,
    };
    ctx.SetTransform(&body_content_translate);
    let indent_started = Instant::now();
    if params.view_options.indent_guides {
        // §25 — drive the guides off the display-row grid so they align
        // with the body under soft-wrap and respect folds. The caret's
        // source line is emphasized via the active brush.
        let active_caret_source_line = selections.first().map(|s| s.head.line as usize);
        paint_indent_guides(
            ctx,
            rope,
            params.frame_display,
            line_height,
            scroll_y,
            viewport_h,
            zero_left,
            params.view_options.indent_size,
            column_advance,
            tab_advance,
            active_caret_source_line,
            brushes.indent_guide,
            brushes.indent_guide_active,
        );
    }
    if params.view_options.whitespace_markers {
        let _ = paint_whitespace_markers(
            ctx,
            factory,
            format,
            rope,
            line_height,
            scroll_y,
            zero_left,
            column_advance,
            first_visible,
            last_visible,
            brushes.line_number,
        );
    }
    timings.indent_guides_us = elapsed_us(indent_started);
    // Gutter paints at absolute x from 0 — restore body-translate.
    ctx.SetTransform(&body_translate);
    let gutter_started = Instant::now();
    if params.view_options.line_numbers {
        // §H3 — compute fold-header info once per frame so the gutter
        // can skip the line numbers of folded body rows. Empty
        // `folded_lines` short-circuits to an empty vec, costing nothing
        // in the common case.
        let fold_headers = compute_fold_headers(
            rope,
            params.view_options.folded_lines,
            params.view_options.markdown_headings,
        );
        // §6b — in caret-only mode also stamp the hovered line's number
        // (muted, like the anchors) so the user can read which line the
        // pointer is on without turning on all numbers.
        let hovered_source_line = params.line_hover.map(|hover| hover.source_line as usize);
        let _ = paint_line_number_gutter(
            ctx,
            factory,
            format,
            rope,
            selections,
            line_height,
            scroll_y,
            viewport_h,
            first_visible,
            last_visible,
            brushes.line_number,
            brushes.line_number_active,
            params.view_options.gutter_caret_line_only,
            params.view_options.relative_line_numbers,
            &fold_headers,
            Some(params.frame_display),
            true,
            hovered_source_line,
        );
        // §H3 — fold triangles share the gutter strip; painted after
        // the line numbers so the glyphs land on top of any current-
        // line backgrounds the gutter painter draws.
        let heading_lines_only: Vec<u32> = params
            .view_options
            .markdown_headings
            .iter()
            .map(|(l, _)| *l)
            .collect();
        // §10 — only show the expanded ▾ ticks when the gutter is hovered;
        // collapsed ▸ ticks always paint (handled inside the painter).
        let gutter_hovered = params.line_hover.is_some_and(|hover| hover.in_gutter);
        let _ = paint_fold_triangles(
            ctx,
            factory,
            format,
            rope,
            params.view_options.folded_lines,
            &heading_lines_only,
            line_height,
            scroll_y,
            first_visible,
            last_visible,
            brushes.line_number,
            brushes.line_number_active,
            gutter_hovered,
        );
    }
    timings.line_numbers_us = elapsed_us(gutter_started);
    Ok(timings)
}

fn elapsed_us(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}
