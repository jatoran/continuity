//! Bottom status bar painter (spec §11 `[ui].show_status_bar`).
//!
//! Phase C1 widened the painter from a fixed `Ln L, Col C` string to a
//! caller-provided list of pre-formatted segments. The ui layer owns the
//! computation; this module just lays them out left-to-right (chips on
//! the right) and paints them. The painter returns a [`StatusBarLayout`]
//! recording each segment's pixel bounds so the click-to-act handler
//! (C2) can hit-test outside of paint.
//!
//! Left segments use **kind-driven slot widths**: each
//! [`StatusBarSegmentKind`] reserves a stable minimum width sized to
//! the widest plausible value of that kind ("Ln 9999, Col 999",
//! "9999 / 9999 lines", etc.). The slot width is
//! `max(estimated_text_width, reserved_min)`, so values smaller than
//! the reservation do not shift later segments as they change. Slots
//! only grow when a value genuinely exceeds the reservation. Chips
//! (right-aligned) stay text-sized — they're additive UI events, not
//! continuously-updating values.
//!
//! Caller passes `top` — the y coord where the bar begins. The bar
//! extends `STATUS_BAR_HEIGHT_DIP` down from that point. The Phase-A5
//! layout reserves a strip below the pane body so the bar sits at
//! `body_height` rather than overlapping the bottom line.
//!
//! Thread ownership: UI thread (caller owns the `ID2D1DeviceContext`).

use windows::core::Interface;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_POINT_2F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1RenderTarget, ID2D1SolidColorBrush, D2D1_DRAW_TEXT_OPTIONS_NONE,
};
use windows::Win32::Graphics::DirectWrite::{IDWriteFactory, IDWriteTextFormat, DWRITE_TEXT_RANGE};

use crate::motion::{StatusTransientDraw, StatusTransientGroup};
use crate::params::Rgba;
use crate::Error;

/// Status bar height in DIPs.
pub const STATUS_BAR_HEIGHT_DIP: f32 = 22.0;

/// Padding between adjacent segments, in DIPs.
const SEGMENT_GAP_DIP: f32 = 16.0;

/// Inner left/right padding for the bar text run, in DIPs.
const BAR_EDGE_PAD_DIP: f32 = 8.0;

/// Phase C1 kind tag — identifies which segment a click-rect belongs to.
/// Mirrors [`continuity_config::StatusBarSegment`] but lives in the
/// render crate so it can travel inside the painter's layout output
/// without pulling `config` into `render`.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum StatusBarSegmentKind {
    /// `Ln L, Col C` for the primary caret.
    Position,
    /// Total character / word / line / byte counter (kind switches via
    /// the C2 cycle action — display label changes but the kind tag
    /// stays `Chars`).
    Chars,
    /// Word count.
    Words,
    /// `non_empty / total` line count.
    Lines,
    /// Selection char / word / line stats.
    Selection,
    /// Live numeric sum of selected numeric tokens.
    NumericSum,
    /// Source-file encoding label.
    Encoding,
    /// Source-file line-ending label.
    LineEndings,
    /// Buffer language tag (`plain` | `markdown`).
    Language,
    /// δ.2 — "idle Xm ago" indicator. Painted alongside other segments
    /// when the editor has been quiet long enough.
    IdleStale,
    /// Phase C3: mixed line-endings or mixed-indent warning chip.
    /// Painted with warn coloring on the right.
    Chip,
    /// One-shot informational chip supplied by the UI. Painted through
    /// the same right-aligned chip lane as warning chips but click is a
    /// no-op.
    NoticeChip,
    /// α.1 persistence-queue depth chip. Only painted while the persist
    /// thread has uncommitted bytes; vanishes when the queue drains.
    /// Click is a no-op — the chip is purely a confidence cue.
    PersistQueueChip,
}

/// One paintable segment.
#[derive(Clone, Debug)]
pub struct StatusBarSegmentDraw {
    /// Pre-formatted text.
    pub text: String,
    /// Kind tag — drives hit-test routing in the click handler.
    pub kind: StatusBarSegmentKind,
    /// Hint shown on hover (C2 follow-up — wiring TBD).
    pub hover: Option<String>,
    /// Paint opacity. `1.0` is fully visible; transient chips lower this
    /// during their fade-out window.
    pub alpha: f32,
}

/// Theme-derived status bar colors.
#[derive(Copy, Clone, Debug, Default)]
pub struct StatusBarColors {
    /// `status.background` — strip fill.
    pub bg: Rgba,
    /// `status.foreground` — default segment text.
    pub fg: Rgba,
    /// `status.warn` — chip foreground for non-blocking warnings.
    pub warn: Rgba,
    /// `status.error` — chip foreground for errors.
    pub error: Rgba,
}

/// All data the painter needs in one struct so the renderer call site
/// stays compact.
#[derive(Clone, Debug)]
pub struct StatusBarData<'a> {
    /// Left-aligned segments, in paint order.
    pub segments: &'a [StatusBarSegmentDraw],
    /// Right-aligned chips (warnings). Painted right-to-left from the
    /// viewport's right edge.
    pub chips: &'a [StatusBarSegmentDraw],
    /// Theme colors.
    pub colors: StatusBarColors,
    /// Localized transient overlays for values that changed this frame.
    pub transients: &'a [StatusTransientDraw],
}

/// Pixel bounds of one rendered segment — used for click hit-testing.
#[derive(Copy, Clone, Debug)]
pub struct SegmentBounds {
    /// Left edge in viewport DIPs.
    pub left: f32,
    /// Right edge in viewport DIPs.
    pub right: f32,
    /// Kind tag identifying the action.
    pub kind: StatusBarSegmentKind,
}

/// Per-frame layout result. Indices into the input slices follow the
/// paint order (left segments first, then right chips). Empty when the
/// status bar was not painted this frame.
#[derive(Clone, Debug, Default)]
pub struct StatusBarLayout {
    /// Y top of the bar in viewport DIPs.
    pub top: f32,
    /// Per-segment bounds, in paint order (left segments first, chips
    /// last). Use [`SegmentBounds::kind`] to dispatch on click.
    pub bounds: Vec<SegmentBounds>,
}

/// Pre-measure a segment's width without a live D2D context. The render
/// pass uses this estimate to lay out hit-rects deterministically. The
/// monospace approximation is good enough for click targets — pixel-
/// perfect layout is the painter's job.
#[must_use]
pub fn estimate_segment_width_dip(text: &str, font_size_dip: f32) -> f32 {
    // Matches the approximate column advance the renderer uses for the
    // soft-wrap projection (`scaled_font_size * 0.55`). Status-bar text
    // is rendered in the same prose font.
    let advance = font_size_dip * 0.55;
    (text.chars().count() as f32) * advance
}

/// Reserved minimum slot width (in character cells) for one segment
/// kind. Sized to the widest plausible value of that kind so the slot
/// origin stays stable as the value updates. Returns `0` for kinds
/// that should hug their text — chips and transient appear/vanish
/// events whose layout shift is part of the signal.
#[must_use]
pub fn min_slot_width_chars(kind: StatusBarSegmentKind) -> u8 {
    match kind {
        // "Ln 9999, Col 999" — 16 chars covers four-digit line × three-digit column.
        StatusBarSegmentKind::Position => 16,
        // "999999 chars" / "999999 words" — six-digit count + label.
        StatusBarSegmentKind::Chars | StatusBarSegmentKind::Words => 14,
        // "9999 / 9999 lines"
        StatusBarSegmentKind::Lines => 18,
        // "Sel 9999c · 9999w · 999l"
        StatusBarSegmentKind::Selection => 24,
        // "Σ 999999.999999"
        StatusBarSegmentKind::NumericSum => 16,
        // "UTF-8" / "windows-1252"
        StatusBarSegmentKind::Encoding => 12,
        // "CRLF" / "LF" / "mixed"
        StatusBarSegmentKind::LineEndings => 6,
        // "markdown" / "plain"
        StatusBarSegmentKind::Language => 9,
        // "idle 99m ago"
        StatusBarSegmentKind::IdleStale => 12,
        // Chips appear and vanish; sized to text.
        StatusBarSegmentKind::Chip
        | StatusBarSegmentKind::NoticeChip
        | StatusBarSegmentKind::PersistQueueChip => 0,
    }
}

/// Slot width in DIPs — `max(estimated_text_width, reserved_min)`.
fn slot_width_dip(kind: StatusBarSegmentKind, text: &str, font_size_dip: f32) -> f32 {
    let advance = font_size_dip * 0.55;
    let reserved = (min_slot_width_chars(kind) as f32) * advance;
    estimate_segment_width_dip(text, font_size_dip).max(reserved)
}

/// Build a [`StatusBarLayout`] for the supplied segment data without
/// painting. Used by both the painter (so paint and hit-test agree)
/// and the click-handler (which calls it directly to recompute on
/// resize).
#[must_use]
pub fn compute_layout(
    data: &StatusBarData<'_>,
    viewport_w: f32,
    top: f32,
    font_size_dip: f32,
) -> StatusBarLayout {
    let mut bounds: Vec<SegmentBounds> = Vec::with_capacity(data.segments.len() + data.chips.len());

    let mut cursor = BAR_EDGE_PAD_DIP;
    for seg in data.segments {
        let w = slot_width_dip(seg.kind, &seg.text, font_size_dip);
        // Snap each segment's left/right to whole DIPs. The X
        // coordinates accumulate across segments via `cursor` and
        // every `slot_width_dip` adds `chars * font_size * 0.55`,
        // which is generally fractional. Drawing text at a
        // fractional X feeds ClearType a different sub-pixel
        // grid every time the slot-text length changes — perceived
        // as "blurry digits" on the live counters (chars / line /
        // col / words). Rounding pins each segment to a stable
        // integer pixel column; same-text repaints become
        // pixel-identical and digit transitions only re-rasterise
        // the slot whose text actually changed.
        let left = cursor.round();
        let right = ((cursor + w).min(viewport_w)).round();
        bounds.push(SegmentBounds {
            left,
            right,
            kind: seg.kind,
        });
        cursor = right + SEGMENT_GAP_DIP;
    }

    let mut right_cursor = (viewport_w - BAR_EDGE_PAD_DIP).max(0.0).round();
    for chip in data.chips {
        let w = estimate_segment_width_dip(&chip.text, font_size_dip);
        let right = right_cursor;
        let left = (right - w).max(0.0).round();
        bounds.push(SegmentBounds {
            left,
            right,
            kind: chip.kind,
        });
        right_cursor = (left - SEGMENT_GAP_DIP).max(0.0);
    }

    StatusBarLayout { top, bounds }
}

/// Paint the bottom status bar across the full viewport width.
///
/// Returns the [`StatusBarLayout`] describing each segment's pixel
/// bounds so the C2 click handler can hit-test against the same x
/// positions the painter used.
///
/// # Errors
///
/// Returns [`Error::Win`] on DirectWrite text-layout creation failure.
#[allow(clippy::too_many_arguments)]
pub fn paint_status_bar(
    ctx: &ID2D1DeviceContext,
    factory: &IDWriteFactory,
    format: &IDWriteTextFormat,
    data: &StatusBarData<'_>,
    viewport_w: f32,
    top: f32,
    font_size_dip: f32,
    fg_brush: &ID2D1SolidColorBrush,
    bg_brush: &ID2D1SolidColorBrush,
    warn_brush: &ID2D1SolidColorBrush,
) -> Result<StatusBarLayout, Error> {
    paint_status_bar_background(ctx, viewport_w, top, bg_brush);
    paint_status_bar_text(
        ctx,
        factory,
        format,
        data,
        viewport_w,
        top,
        font_size_dip,
        fg_brush,
        warn_brush,
    )
}

/// Paint only the retained status-bar shell background.
pub(crate) fn paint_status_bar_background(
    ctx: &ID2D1DeviceContext,
    viewport_w: f32,
    top: f32,
    bg_brush: &ID2D1SolidColorBrush,
) {
    let bottom = top + STATUS_BAR_HEIGHT_DIP;
    let bar = D2D_RECT_F {
        left: 0.0,
        top,
        right: viewport_w,
        bottom,
    };
    unsafe { ctx.FillRectangle(&bar, bg_brush) };
}

#[allow(clippy::too_many_arguments)]
fn paint_status_bar_text(
    ctx: &ID2D1DeviceContext,
    factory: &IDWriteFactory,
    format: &IDWriteTextFormat,
    data: &StatusBarData<'_>,
    viewport_w: f32,
    top: f32,
    font_size_dip: f32,
    fg_brush: &ID2D1SolidColorBrush,
    warn_brush: &ID2D1SolidColorBrush,
) -> Result<StatusBarLayout, Error> {
    let layout = compute_layout(data, viewport_w, top, font_size_dip);
    let mut iter_idx = 0;
    for seg in data.segments {
        let b = layout.bounds[iter_idx];
        iter_idx += 1;
        draw_segment(
            ctx,
            factory,
            format,
            &seg.text,
            b.left,
            top,
            (b.right - b.left).max(1.0),
            font_size_dip,
            fg_brush,
            seg.alpha,
        )?;
    }
    for chip in data.chips {
        let b = layout.bounds[iter_idx];
        iter_idx += 1;
        draw_segment(
            ctx,
            factory,
            format,
            &chip.text,
            b.left,
            top,
            (b.right - b.left).max(1.0),
            font_size_dip,
            warn_brush,
            chip.alpha,
        )?;
    }
    paint_transients(
        ctx,
        factory,
        format,
        data,
        &layout,
        top,
        font_size_dip,
        fg_brush,
        warn_brush,
    )?;
    Ok(layout)
}

#[allow(clippy::too_many_arguments)]
fn paint_transients(
    ctx: &ID2D1DeviceContext,
    factory: &IDWriteFactory,
    format: &IDWriteTextFormat,
    data: &StatusBarData<'_>,
    layout: &StatusBarLayout,
    top: f32,
    font_size_dip: f32,
    fg_brush: &ID2D1SolidColorBrush,
    warn_brush: &ID2D1SolidColorBrush,
) -> Result<(), Error> {
    if data.transients.is_empty() {
        return Ok(());
    }
    let render_target: ID2D1RenderTarget = ctx.cast()?;
    for transient in data.transients {
        let (text, bound_index, base) = match transient.group {
            StatusTransientGroup::Segment => {
                let Some(seg) = data.segments.get(transient.index) else {
                    continue;
                };
                (seg.text.as_str(), transient.index, data.colors.fg)
            }
            StatusTransientGroup::Chip => {
                let Some(chip) = data.chips.get(transient.index) else {
                    continue;
                };
                (
                    chip.text.as_str(),
                    data.segments.len() + transient.index,
                    data.colors.warn,
                )
            }
        };
        let Some(bounds) = layout.bounds.get(bound_index).copied() else {
            continue;
        };
        let alpha = transient.alpha.clamp(0.0, 1.0);
        if alpha <= f32::EPSILON {
            continue;
        }
        let color = Rgba {
            a: base.a * alpha,
            ..base
        };
        let brush =
            unsafe { render_target.CreateSolidColorBrush(&D2D1_COLOR_F::from(color), None)? };
        let fallback = match transient.group {
            StatusTransientGroup::Segment => fg_brush,
            StatusTransientGroup::Chip => warn_brush,
        };
        let brush = if color.a > f32::EPSILON {
            &brush
        } else {
            fallback
        };
        draw_segment(
            ctx,
            factory,
            format,
            text,
            bounds.left,
            top + transient.translate_y_dip,
            (bounds.right - bounds.left).max(1.0),
            font_size_dip,
            brush,
            1.0,
        )?;
    }
    Ok(())
}

/// Convenience wrapper for the per-frame status text pass. The
/// background strip is retained in the static chrome command list.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_status_bar_frame_text<F>(
    ctx: &ID2D1DeviceContext,
    factory: &IDWriteFactory,
    format: &IDWriteTextFormat,
    data: &StatusBarData<'_>,
    viewport_w: f32,
    client_height_dip: f32,
    font_size_dip: f32,
    make_brush: &F,
) where
    F: Fn(Rgba) -> Result<ID2D1SolidColorBrush, Error>,
{
    let Ok(foreground_brush) = make_brush(data.colors.fg) else {
        return;
    };
    let Ok(warning_brush) = make_brush(data.colors.warn) else {
        return;
    };
    let top = (client_height_dip - STATUS_BAR_HEIGHT_DIP).max(0.0);
    let _ = paint_status_bar_text(
        ctx,
        factory,
        format,
        data,
        viewport_w,
        top,
        font_size_dip,
        &foreground_brush,
        &warning_brush,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_segment(
    ctx: &ID2D1DeviceContext,
    factory: &IDWriteFactory,
    format: &IDWriteTextFormat,
    text: &str,
    left: f32,
    top: f32,
    width: f32,
    font_size_dip: f32,
    brush: &ID2D1SolidColorBrush,
    alpha: f32,
) -> Result<(), Error> {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let layout = unsafe { factory.CreateTextLayout(&wide, format, width, STATUS_BAR_HEIGHT_DIP)? };
    // The supplied `format` is the body text format, whose size tracks
    // body zoom. The status bar is chrome — pin its glyph size to the
    // caller-supplied (unscaled) size so Ctrl+wheel zoom never clips the
    // fixed-height bar. Width estimates in `compute_layout` already use
    // the same size, so glyphs and slot widths agree.
    if font_size_dip > 0.0 {
        let full_range = DWRITE_TEXT_RANGE {
            startPosition: 0,
            length: wide.len() as u32,
        };
        unsafe {
            let _ = layout.SetFontSize(font_size_dip, full_range);
        }
    }
    let alpha = alpha.clamp(0.0, 1.0);
    unsafe {
        brush.SetOpacity(alpha);
        ctx.DrawTextLayout(
            D2D_POINT_2F { x: left, y: top },
            &layout,
            brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
        );
        brush.SetOpacity(1.0);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
