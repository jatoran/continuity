//! Phase-16.5 spell-squiggle paint helper.
//!
//! Lives next to `renderer.rs` to keep that file under the 600-line
//! conventions cap. The renderer iterates [`crate::SpellSquiggleSpan`]s
//! and asks this module to draw the wavy underline for each one.
//!
//! Thread ownership: caller is the UI thread (the only owner of the D2D
//! context).

use continuity_layout::LayoutCache;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1RenderTarget, ID2D1SolidColorBrush,
};

use crate::params::DrawParams;
use crate::text_helpers::{build_key_for_spec, hit_test_x, utf8_byte_to_utf16_index};
use crate::Error;

/// Paint every [`crate::SpellSquiggleSpan`] in `params.spell_spans`
/// against the per-line cached layouts. Skips spans whose line is
/// outside the visible window. No-op when `params.spell_spans` is empty.
///
/// Phase 17.6 cleanup: source-byte ranges are routed through
/// [`crate::FrameDisplay::source_byte_in_line_to_display_utf16`] so a
/// misspelled word in a line with hidden markers underlines the *displayed*
/// glyphs — `**typo**` underlines `typo`, not what used to be at six
/// character widths past the start.
///
/// # Errors
///
/// Returns [`Error::Graphics`] when D2D brush creation fails.
///
/// # Safety
///
/// `ctx`, `render_target`, and `cache` must be alive for the call;
/// caller wraps in a `BeginDraw`/`EndDraw` block.
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn paint_spell_spans(
    ctx: &ID2D1DeviceContext,
    render_target: &ID2D1RenderTarget,
    cache: &mut LayoutCache,
    params: &DrawParams<'_>,
    margins_left: f32,
    line_height: f32,
    scroll_y: f32,
    wrap_width_dip: u32,
    first_visible: usize,
    last_visible: usize,
) -> Result<(), Error> {
    if params.spell_spans.is_empty() {
        return Ok(());
    }
    let squiggle_color = D2D1_COLOR_F {
        r: 0.85,
        g: 0.15,
        b: 0.15,
        a: 1.0,
    };
    let squiggle_brush: ID2D1SolidColorBrush =
        render_target.CreateSolidColorBrush(&squiggle_color, None)?;
    let fd = params.frame_display;
    for span in params.spell_spans {
        let line_idx = span.line as usize;
        if line_idx < first_visible || line_idx >= last_visible {
            continue;
        }
        let Some(spec) = fd.line(line_idx) else {
            continue;
        };
        let key = build_key_for_spec(spec, params.document, params.font_state, wrap_width_dip);
        let Some(entry) = cache.get(&key) else {
            continue;
        };
        let span_start = span.byte_in_line_start as usize;
        let span_end = span.byte_in_line_end as usize;
        let utf16_start = fd
            .source_byte_in_line_to_display_utf16(line_idx, span_start)
            .map(|u| u as usize)
            .unwrap_or_else(|| utf8_byte_to_utf16_index(entry.text, span_start));
        let utf16_end = fd
            .source_byte_in_line_to_display_utf16(line_idx, span_end)
            .map(|u| u as usize)
            .unwrap_or_else(|| utf8_byte_to_utf16_index(entry.text, span_end));
        let x_start = hit_test_x(entry.layout, utf16_start).unwrap_or(0.0) + margins_left;
        let x_end = hit_test_x(entry.layout, utf16_end).unwrap_or(x_start) + margins_left;
        if x_end <= x_start {
            continue;
        }
        let y_top = line_idx as f32 * line_height - scroll_y + line_height - 3.0;
        paint_squiggle(ctx, &squiggle_brush, x_start, x_end, y_top);
    }
    Ok(())
}

/// Paint a small wavy red underline between `x_start` and `x_end` with
/// its baseline at `y_top`. Uses `FillRectangle` rather than a path
/// geometry — keeps the renderer free of additional D2D-factory plumbing
/// and avoids per-frame geometry allocations.
///
/// # Safety
///
/// `ctx` and `brush` must be alive for the duration of the call. The
/// renderer's `BeginDraw` block is the only legitimate caller.
pub(crate) unsafe fn paint_squiggle(
    ctx: &ID2D1DeviceContext,
    brush: &ID2D1SolidColorBrush,
    x_start: f32,
    x_end: f32,
    y_top: f32,
) {
    // Step + amplitude tuned so a typical 5-letter word shows ~3 humps.
    let step: f32 = 3.0;
    let amp: f32 = 1.5;
    let dot_w: f32 = 1.0;
    let dot_h: f32 = 1.5;
    let mut x = x_start;
    let mut idx: u32 = 0;
    while x < x_end {
        let y = if idx.is_multiple_of(2) {
            y_top
        } else {
            y_top + amp
        };
        let right = (x + dot_w).min(x_end);
        let rect = D2D_RECT_F {
            left: x,
            top: y,
            right,
            bottom: y + dot_h,
        };
        ctx.FillRectangle(&rect, brush);
        x += step;
        idx = idx.wrapping_add(1);
    }
}
