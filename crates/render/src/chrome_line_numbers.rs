//! Line-number gutter painter — sibling to [`crate::chrome`] so the
//! `chrome.rs` file stays under the conventions cap. The gutter shares
//! the gutter metrics defined in `chrome.rs`; the function itself is
//! self-contained.
//!
//! Thread ownership: caller is the UI thread.

use ropey::Rope;
use windows::core::Interface;
use windows::Win32::Graphics::Direct2D::Common::{D2D_POINT_2F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1RenderTarget, ID2D1SolidColorBrush, D2D1_DRAW_TEXT_OPTIONS_NONE,
};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout, DWRITE_TEXT_ALIGNMENT_TRAILING,
};

use crate::chrome::gutter_width_for_line_count;
use crate::display_projection::FrameDisplay;
use crate::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct GutterLabel {
    display_row: u32,
    source_line: usize,
    is_active: bool,
}

/// Width passed to `CreateTextLayout` for a gutter label: the column
/// width minus the fold-icon / breathing-room gap reserved on the right
/// ([`crate::chrome::gutter_fold_gap_dip`]), floored at 1 DIP so
/// DirectWrite never sees a non-positive max width when the font is tiny.
/// Trailing-aligned digits therefore end exactly at the left edge of the
/// fold-icon gap — the icons sit to their right and never overlap.
fn label_layout_width(gutter_width: f32, font_size_dip: f32) -> f32 {
    (gutter_width - crate::chrome::gutter_fold_gap_dip(font_size_dip)).max(1.0)
}

/// Apply `TRAILING` alignment to a freshly-built gutter layout so the
/// digits hug the right edge of the layout box (which is inset from the
/// divider by the fold-icon gap, [`crate::chrome::gutter_fold_gap_dip`])
/// instead of left-flowing from x=0. Cheap: one DirectWrite call per
/// layout.
fn align_trailing(layout: &IDWriteTextLayout) {
    let _ = unsafe { layout.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING) };
}

/// Multiplier applied to the line-number color's alpha when painting the
/// gutter↔body vertical rule. Reduces a hard divider line that was the
/// loudest piece of every pane's chrome — especially in 2×2 grids where
/// the rules pile up — without removing it (full transparency reads as a
/// rendering bug).
const GUTTER_DIVIDER_ALPHA_MULTIPLIER: f32 = 0.35;

/// Build a faded-alpha brush from the line-number brush's color. Returns
/// `None` if the brush cannot be derived (the caller falls back to the
/// unfaded brush so the divider remains visible).
fn make_divider_brush(
    ctx: &ID2D1DeviceContext,
    source: &ID2D1SolidColorBrush,
) -> Option<ID2D1SolidColorBrush> {
    let render_target: ID2D1RenderTarget = ctx.cast().ok()?;
    let mut color = unsafe { source.GetColor() };
    color.a *= GUTTER_DIVIDER_ALPHA_MULTIPLIER;
    unsafe { render_target.CreateSolidColorBrush(&color, None).ok() }
}

fn compute_visible_row_bounds(
    scroll_y: f32,
    viewport_h: f32,
    line_height: f32,
    total_rows: u32,
) -> Option<(u32, u32)> {
    if total_rows == 0 || viewport_h <= 0.0 || line_height <= 0.0 {
        return None;
    }
    let first = ((scroll_y / line_height).floor() as i64).max(0);
    let last = (((scroll_y + viewport_h) / line_height).ceil() as i64 - 1).max(first);
    let max_row = i64::from(total_rows.saturating_sub(1));
    let first = first.min(max_row) as u32;
    let last = last.min(max_row) as u32;
    Some((first, last))
}

fn push_unique_label(labels: &mut Vec<GutterLabel>, next: GutterLabel) {
    if let Some(existing) = labels.iter_mut().find(|label| {
        label.display_row == next.display_row && label.source_line == next.source_line
    }) {
        existing.is_active |= next.is_active;
        return;
    }
    labels.push(next);
}

fn add_viewport_anchor_labels(
    labels: &mut Vec<GutterLabel>,
    first_row: u32,
    last_row: u32,
    source_line_for_row: impl Fn(u32) -> Option<usize>,
    is_in_fold_body: &dyn Fn(u32) -> bool,
) {
    for display_row in [first_row, last_row] {
        let Some(source_line) = source_line_for_row(display_row) else {
            continue;
        };
        let line_u32 = u32::try_from(source_line).unwrap_or(0);
        if is_in_fold_body(line_u32) {
            continue;
        }
        push_unique_label(
            labels,
            GutterLabel {
                display_row,
                source_line,
                is_active: false,
            },
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_gutter_label(
    ctx: &ID2D1DeviceContext,
    factory: &IDWriteFactory,
    format: &IDWriteTextFormat,
    source_line: usize,
    display_row: u32,
    active_line: Option<usize>,
    fold_body_count: Option<u32>,
    relative: bool,
    gutter_width: f32,
    line_height: f32,
    scroll_y: f32,
    brush: &ID2D1SolidColorBrush,
) -> Result<(), Error> {
    let label = build_label(source_line, active_line, fold_body_count, relative);
    let wide: Vec<u16> = label.encode_utf16().collect();
    let font_size_dip = unsafe { format.GetFontSize() };
    let layout = unsafe {
        factory.CreateTextLayout(
            &wide,
            format,
            label_layout_width(gutter_width, font_size_dip),
            line_height,
        )?
    };
    align_trailing(&layout);
    let y = display_row as f32 * line_height - scroll_y;
    unsafe {
        ctx.DrawTextLayout(
            D2D_POINT_2F { x: 0.0, y },
            &layout,
            brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
        );
    }
    Ok(())
}

/// Paint the line-number gutter column.
///
/// When `caret_only` is `true` (Phase A §A4 default) the gutter renders
/// the caret line plus muted first/last viewport anchors; the gutter strip
/// otherwise stays empty so the always-on gutter doesn't compete with body
/// text for attention. When `false`, every visible line's number is rendered
/// right-aligned with the caret line painted via `active_brush`.
/// When `relative` is `true`, non-caret rows show their distance from
/// the primary caret line; the caret row itself remains absolute.
///
/// When `frame_display` is `Some` the gutter iterates *display rows* (so
/// wrapped paragraphs only get one label, placed on their first row, and
/// the y-grid lines up with the body's display-row paint). Otherwise it
/// falls back to one row per source line.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_line_number_gutter(
    ctx: &ID2D1DeviceContext,
    factory: &IDWriteFactory,
    format: &IDWriteTextFormat,
    rope: &Rope,
    selections: &[continuity_text::Selection],
    line_height: f32,
    scroll_y: f32,
    viewport_h: f32,
    first_visible: usize,
    last_visible: usize,
    inactive_brush: &ID2D1SolidColorBrush,
    active_brush: &ID2D1SolidColorBrush,
    caret_only: bool,
    relative: bool,
    fold_headers: &[crate::chrome_fold::FoldHeaderInfo],
    frame_display: Option<&FrameDisplay>,
    pane_focused: bool,
) -> Result<(), Error> {
    let font_size_dip = unsafe { format.GetFontSize() };
    let gutter_width = gutter_width_for_line_count(font_size_dip, rope.len_lines());
    // Vertical 1-DIP rule separating the gutter from the body text.
    // Half-pixel offset so the rule lands on one device row cleanly
    // under grayscale AA, matching the ruler-columns / indent-guide
    // rendering convention.
    let sep_x = gutter_width.floor() - 0.5;
    let sep_rect = D2D_RECT_F {
        left: sep_x,
        top: 0.0,
        right: sep_x + 1.0,
        bottom: viewport_h.max(0.0),
    };
    // Soften the rule by deriving a low-alpha brush from the line-number
    // color. Fall back to the unfaded brush if D2D refuses the derived
    // one (extreme rarity — solid brush construction is essentially
    // infallible) so the gutter never silently loses its boundary.
    let divider_brush = make_divider_brush(ctx, inactive_brush);
    let divider_ref: &ID2D1SolidColorBrush = divider_brush.as_ref().unwrap_or(inactive_brush);
    unsafe { ctx.FillRectangle(&sep_rect, divider_ref) };

    // Unfocused panes paint the divider and stop — no line numbers, no
    // caret-line emphasis, no stray "1" on empty buffers. Keeps the
    // gutter present (so geometry never shifts when focus changes) but
    // visually quiet in a 2×2 grid.
    if !pane_focused {
        return Ok(());
    }

    let total_lines = rope.len_lines();
    let active_line = selections.first().map(|s| s.head.line as usize);
    // §H3: pre-compute helpers so the gutter loop is O(visible_rows).
    // `body_count_for_header`: if `line_idx` is a fold header, returns
    // the body line count for the "▸ N" indicator.
    // `is_in_fold_body`: true when `line_idx` is hidden inside any
    // active fold; the gutter skips its number entirely.
    let body_count_for_header = |line_idx: u32| -> Option<u32> {
        fold_headers
            .iter()
            .find(|h| h.header_line == line_idx)
            .map(|h| h.body_line_count)
    };
    let is_in_fold_body = |line_idx: u32| -> bool {
        fold_headers
            .iter()
            .any(|h| line_idx > h.header_line && line_idx < h.end_line_exclusive)
    };

    // Display-row branch: when a FrameDisplay is provided we use it as the
    // ground truth for the y-grid, so wrapped paragraphs only get one label
    // (on their first display row) and the labels align with body text.
    if let Some(fd) = frame_display {
        return paint_gutter_display_rows(
            ctx,
            factory,
            format,
            fd,
            active_line,
            line_height,
            scroll_y,
            viewport_h,
            inactive_brush,
            active_brush,
            caret_only,
            relative,
            &body_count_for_header,
            &is_in_fold_body,
            gutter_width,
        );
    }

    if caret_only {
        let Some((first_row, last_row)) =
            compute_visible_row_bounds(scroll_y, viewport_h, line_height, total_lines as u32)
        else {
            return Ok(());
        };
        let mut labels = Vec::with_capacity(3);
        add_viewport_anchor_labels(
            &mut labels,
            first_row,
            last_row,
            |row| Some(row as usize),
            &is_in_fold_body,
        );
        if let Some(line_idx) = active_line {
            let line_u32 = u32::try_from(line_idx).unwrap_or(0);
            if !is_in_fold_body(line_u32) && line_idx >= first_visible && line_idx < last_visible {
                push_unique_label(
                    &mut labels,
                    GutterLabel {
                        display_row: line_u32,
                        source_line: line_idx,
                        is_active: true,
                    },
                );
            }
        }
        for label in labels {
            let line_u32 = u32::try_from(label.source_line).unwrap_or(0);
            let brush = if label.is_active {
                active_brush
            } else {
                inactive_brush
            };
            draw_gutter_label(
                ctx,
                factory,
                format,
                label.source_line,
                label.display_row,
                active_line,
                body_count_for_header(line_u32),
                relative,
                gutter_width,
                line_height,
                scroll_y,
                brush,
            )?;
        }
        return Ok(());
    }
    for line_idx in first_visible..last_visible.min(total_lines) {
        let line_u32 = u32::try_from(line_idx).unwrap_or(0);
        if is_in_fold_body(line_u32) {
            // Hidden by an active fold — skip the gutter number entirely
            // so visible numbers reflect *source* line indices with
            // folded bodies elided (e.g. after line 30 the next visible
            // row reads "32", not "31").
            continue;
        }
        let brush = if active_line == Some(line_idx) {
            active_brush
        } else {
            inactive_brush
        };
        draw_gutter_label(
            ctx,
            factory,
            format,
            line_idx,
            line_u32,
            active_line,
            body_count_for_header(line_u32),
            relative,
            gutter_width,
            line_height,
            scroll_y,
            brush,
        )?;
    }
    Ok(())
}

/// Render a per-row gutter label using the full 1-based line number.
/// The optional `fold_body_count` appends a `▸N`
/// indicator — same suffix the focused-pane fold path emits.
fn build_label(
    line_idx: usize,
    active_line: Option<usize>,
    fold_body_count: Option<u32>,
    relative: bool,
) -> String {
    let number = if relative && active_line.is_some_and(|active| active != line_idx) {
        active_line.map_or(line_idx + 1, |active| active.abs_diff(line_idx))
    } else {
        line_idx + 1
    };
    if let Some(n) = fold_body_count {
        format!("{number} ▸{n}")
    } else {
        number.to_string()
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_gutter_display_rows(
    ctx: &ID2D1DeviceContext,
    factory: &IDWriteFactory,
    format: &IDWriteTextFormat,
    fd: &FrameDisplay,
    active_line: Option<usize>,
    line_height: f32,
    scroll_y: f32,
    viewport_h: f32,
    inactive_brush: &ID2D1SolidColorBrush,
    active_brush: &ID2D1SolidColorBrush,
    caret_only: bool,
    relative: bool,
    body_count_for_header: &dyn Fn(u32) -> Option<u32>,
    is_in_fold_body: &dyn Fn(u32) -> bool,
    gutter_width: f32,
) -> Result<(), Error> {
    let total_dl = fd.display_line_count();
    if total_dl == 0 {
        return Ok(());
    }
    let Some((first_anchor_dl, last_anchor_dl)) =
        compute_visible_row_bounds(scroll_y, viewport_h, line_height, total_dl)
    else {
        return Ok(());
    };
    let first_dl = ((scroll_y / line_height).floor() as i64).max(0);
    let last_dl =
        (((scroll_y + viewport_h) / line_height).ceil() as i64 + 1).clamp(0, i64::from(total_dl));

    if caret_only {
        let mut labels = Vec::with_capacity(3);
        add_viewport_anchor_labels(
            &mut labels,
            first_anchor_dl,
            last_anchor_dl,
            |row| {
                fd.display_line_by_index(row)
                    .map(|spec| spec.source_line.raw() as usize)
            },
            is_in_fold_body,
        );
        if let Some(source_line) = active_line {
            let line_u32 = u32::try_from(source_line).unwrap_or(0);
            if !is_in_fold_body(line_u32) {
                let dl_idx = fd.first_display_line_index_for_source(source_line);
                if dl_idx >= first_anchor_dl && dl_idx < last_dl as u32 {
                    push_unique_label(
                        &mut labels,
                        GutterLabel {
                            display_row: dl_idx,
                            source_line,
                            is_active: true,
                        },
                    );
                }
            }
        }
        for label in labels {
            let line_u32 = u32::try_from(label.source_line).unwrap_or(0);
            let brush = if label.is_active {
                active_brush
            } else {
                inactive_brush
            };
            draw_gutter_label(
                ctx,
                factory,
                format,
                label.source_line,
                label.display_row,
                active_line,
                body_count_for_header(line_u32),
                relative,
                gutter_width,
                line_height,
                scroll_y,
                brush,
            )?;
        }
        return Ok(());
    }

    for dl_idx in first_dl..last_dl {
        let Some(spec) = fd.display_line_by_index(dl_idx as u32) else {
            continue;
        };
        if spec.is_wrap_continuation {
            continue;
        }
        let source_line = spec.source_line.raw() as usize;
        let line_u32 = u32::try_from(source_line).unwrap_or(0);
        if is_in_fold_body(line_u32) {
            continue;
        }
        let brush = if active_line == Some(source_line) {
            active_brush
        } else {
            inactive_brush
        };
        draw_gutter_label(
            ctx,
            factory,
            format,
            source_line,
            dl_idx as u32,
            active_line,
            body_count_for_header(line_u32),
            relative,
            gutter_width,
            line_height,
            scroll_y,
            brush,
        )?;
    }
    Ok(())
}
