//! F3 + F4 per-line paint dispatchers.
//!
//! Three thin wrappers around the underlying per-line painters
//! ([`crate::inline_color_paint`], [`crate::table_formula_paint`]):
//!
//! - [`paint_inline_color_pre_text`] paints highlight rectangles
//!   **before** the main `DrawTextLayout` so the rect sits behind glyphs.
//! - [`paint_inline_color_post_text`] paints hex-color foreground
//!   overdraws **after** the main `DrawTextLayout` so the colored
//!   glyphs cover the default-color glyphs.
//! - [`paint_table_overrides_post_text`] paints the F4 cell swap-in
//!   text on top of formula source bytes.
//!
//! All three compute the per-line byte range and caret-byte set once
//! and bail early when their respective span slice is empty, keeping
//! the renderer's per-line loop short.
//!
//! The F4 *visual-table* chrome (cell borders, header / alignment-row
//! backgrounds, per-column-aligned cell text) used to ride the same
//! per-line dispatch surface. P14.1 moved it to a retained per-table
//! command-list cache (`crate::table_chrome_cache`,
//! `crate::renderer_table_chrome`).

use windows::Win32::Graphics::Direct2D::{ID2D1DeviceContext, ID2D1SolidColorBrush};
use windows::Win32::Graphics::DirectWrite::{IDWriteFactory, IDWriteTextLayout};

use crate::inline_color_paint::{
    caret_bytes_from_selections, paint_inline_color_backgrounds_line,
    paint_inline_color_foregrounds_line,
};
use crate::params::DrawParams;
use crate::table_formula_paint::{paint_table_overrides_line, TableFormulaBrushes};

/// F3 — paint inline highlight backgrounds (`==text==`) for this line.
/// Called **before** the main `DrawTextLayout` so the fill sits behind
/// glyphs. No-op when `params.inline_color_spans` is empty.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_inline_color_pre_text(
    ctx: &ID2D1DeviceContext,
    layout: &IDWriteTextLayout,
    entry_text: &str,
    rope: &ropey::Rope,
    selections: &[continuity_text::Selection],
    params: &DrawParams<'_>,
    line_idx: usize,
    line_height: f32,
    highlight_bg_brush: &ID2D1SolidColorBrush,
) {
    if params.inline_color_spans.is_empty() {
        return;
    }
    let (line_start, line_end) = line_byte_range(rope, line_idx);
    let caret_bytes = caret_bytes_from_selections(rope, selections);
    paint_inline_color_backgrounds_line(
        ctx,
        layout,
        entry_text,
        params.frame_display,
        line_idx,
        line_start..line_end,
        line_height,
        params.inline_color_spans,
        &caret_bytes,
        highlight_bg_brush,
    );
}

/// F3 — paint hex-color foreground overdraws (`{#hex:text}`) for this
/// line. Called **after** the main `DrawTextLayout` so the colored
/// glyphs cover the default-color glyphs. No-op when
/// `params.inline_color_spans` is empty.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_inline_color_post_text(
    ctx: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    layout: &IDWriteTextLayout,
    entry_text: &str,
    rope: &ropey::Rope,
    selections: &[continuity_text::Selection],
    params: &DrawParams<'_>,
    line_idx: usize,
    line_height: f32,
) {
    if params.inline_color_spans.is_empty() {
        return;
    }
    let (line_start, line_end) = line_byte_range(rope, line_idx);
    let caret_bytes = caret_bytes_from_selections(rope, selections);
    paint_inline_color_foregrounds_line(
        ctx,
        dwrite,
        params.format,
        layout,
        entry_text,
        params.frame_display,
        line_idx,
        line_start..line_end,
        line_height,
        params.inline_color_spans,
        &caret_bytes,
    );
}

/// F4 — paint cell-display swap-in text on top of formula source bytes.
/// No-op when `params.table_overrides` is empty.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_table_overrides_post_text(
    ctx: &ID2D1DeviceContext,
    dwrite: &IDWriteFactory,
    layout: &IDWriteTextLayout,
    entry_text: &str,
    rope: &ropey::Rope,
    params: &DrawParams<'_>,
    line_idx: usize,
    line_height: f32,
    brushes: TableFormulaBrushes<'_>,
) {
    if params.table_overrides.is_empty() {
        return;
    }
    let (line_start, line_end) = line_byte_range(rope, line_idx);
    paint_table_overrides_line(
        ctx,
        dwrite,
        params.format,
        layout,
        entry_text,
        params.frame_display,
        line_idx,
        line_start..line_end,
        line_height,
        params.table_overrides,
        params.table_layouts,
        &brushes,
    );
}

fn line_byte_range(rope: &ropey::Rope, line_idx: usize) -> (usize, usize) {
    let line_start = rope.line_to_byte(line_idx);
    let line_end = if line_idx + 1 < rope.len_lines() {
        rope.line_to_byte(line_idx + 1)
    } else {
        rope.len_bytes()
    };
    (line_start, line_end)
}
