//! Indent-guide painting for the editor body chrome. Lives in its own
//! file so `chrome.rs` stays under the 600-line cap; it is otherwise a
//! natural part of the view-toggle drawing surface in [`crate::chrome`].
//!
//! Thread ownership: caller is the UI thread (the only owner of the
//! `ID2D1DeviceContext` passed in).

use ropey::Rope;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{ID2D1DeviceContext, ID2D1SolidColorBrush};

use crate::chrome::ContentMargins;
use crate::display_projection::FrameDisplay;

/// Paint vertical indent-guide rules aligned with the first character
/// of each enclosing parent indent level. Tabs and spaces are measured
/// at their actual rendered advance so guides land under the visible
/// leading whitespace boundaries, not under a `space_advance * column`
/// approximation.
///
/// §25 — iterates the **display-row** grid (via `frame_display`), not the
/// source-line grid: each visible display row is mapped back to its
/// source line, wrap-continuation rows are skipped (a wrapped paragraph
/// only draws guides on its first row), and each guide paints at
/// `display_row * line_height - scroll_y`. This keeps the guides aligned
/// with the body text under soft-wrap and respects folds / row
/// reservations because only rows the display map actually projects are
/// iterated. The body-left-edge guide (`k == 0`, x at the text column
/// origin) is suppressed — it duplicated the gutter↔body divider and read
/// as a spurious guide.
///
/// When `active_caret_source_line` is `Some`, that source line's deepest
/// guide column is drawn with `active_color` so the indent level the
/// caret sits in is emphasized.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_indent_guides(
    ctx: &ID2D1DeviceContext,
    rope: &Rope,
    frame_display: &FrameDisplay,
    line_height: f32,
    scroll_y: f32,
    viewport_h: f32,
    margins: ContentMargins,
    indent_size: u32,
    column_advance: f32,
    tab_advance: f32,
    active_caret_source_line: Option<usize>,
    color: &ID2D1SolidColorBrush,
    active_color: &ID2D1SolidColorBrush,
) {
    if indent_size == 0 || column_advance <= 0.0 || line_height <= 0.0 {
        return;
    }
    let tab_advance = if tab_advance > 0.0 {
        tab_advance
    } else {
        column_advance * indent_size as f32
    };
    let total_lines = rope.len_lines();
    let total_rows = frame_display.display_line_count();
    if total_lines == 0 || total_rows == 0 {
        return;
    }

    // Visible display-row window with a one-row over-scan top and bottom
    // so guides do not pop at the viewport edges during a scroll.
    let first_row = ((scroll_y / line_height).floor() as i64 - 1).max(0) as u32;
    let last_row = ((((scroll_y + viewport_h) / line_height).ceil() as i64) + 1)
        .clamp(0, i64::from(total_rows)) as u32;
    if first_row >= last_row {
        return;
    }

    // Resolve the source-line span covered by the visible display rows so
    // the blank-line carry-over has a non-blank anchor on each flank.
    let first_source = source_line_for_row(frame_display, first_row).unwrap_or(0);
    let last_source = source_line_for_row(frame_display, last_row.saturating_sub(1))
        .map_or(total_lines, |s| (s + 1).min(total_lines));

    // Phase 17.6 intelligent indent guides: extend the scanned source
    // window by a small skirt so the blank-line carry-over reaches a
    // non-blank anchor when editing near a paragraph boundary.
    const SKIRT: usize = 64;
    let scan_start = first_source.saturating_sub(SKIRT);
    let scan_end = (last_source + SKIRT).min(total_lines);
    if scan_start >= scan_end {
        return;
    }

    // Per-scanned-source-line indent-unit boundary x-positions in DIPs
    // from `margins.left`. `None` marks a blank line (only whitespace).
    let mut raw: Vec<Option<Vec<f32>>> = Vec::with_capacity(scan_end - scan_start);
    for line_idx in scan_start..scan_end {
        raw.push(measure_indent_boundaries(
            rope,
            line_idx,
            indent_size,
            column_advance,
            tab_advance,
        ));
    }
    // Carry the boundary list of the previous non-blank line forward.
    let mut forward = raw.clone();
    let mut last_bounds: Option<Vec<f32>> = None;
    for slot in forward.iter_mut() {
        match slot {
            Some(v) => last_bounds = Some(v.clone()),
            None => *slot = last_bounds.clone(),
        }
    }
    // Carry the boundary list of the next non-blank line backward.
    let mut backward = raw.clone();
    let mut next_bounds: Option<Vec<f32>> = None;
    for slot in backward.iter_mut().rev() {
        match slot {
            Some(v) => next_bounds = Some(v.clone()),
            None => *slot = next_bounds.clone(),
        }
    }

    for display_row in first_row..last_row {
        let Some(spec) = frame_display.display_line_by_index(display_row) else {
            continue;
        };
        // A wrapped paragraph draws its guides once, on the first display
        // row of the source line; continuation rows inherit the column
        // but would otherwise double-stamp.
        if spec.is_wrap_continuation {
            continue;
        }
        let source_line = spec.source_line.raw() as usize;
        if source_line < scan_start || source_line >= scan_end {
            continue;
        }
        let local = source_line - scan_start;
        // Resolve this source line's effective boundary set:
        // - non-blank → its own measured boundaries.
        // - blank with both flanks → forward neighbour truncated to the
        //   shorter flank (parents shared on both sides).
        // - edge of buffer → skip.
        let (bounds, depth): (&[f32], usize) =
            match (&raw[local], &forward[local], &backward[local]) {
                (Some(v), _, _) => (v.as_slice(), v.len()),
                (None, Some(f), Some(b)) => {
                    let n = f.len().min(b.len());
                    (&f[..n], n)
                }
                _ => continue,
            };
        if depth == 0 {
            continue;
        }
        let y = display_row as f32 * line_height - scroll_y;
        let is_active_line = active_caret_source_line == Some(source_line);
        // A guide at offset C means "an enclosing parent's content starts
        // at C". For depth N the parents sit at 0, bounds[0], …,
        // bounds[N-2]. The k == 0 column (the body's own left edge) is
        // suppressed — it duplicated the gutter divider.
        for k in 1..depth {
            let col_x = bounds[k - 1];
            // Half-pixel offset so the 1-DIP rule hits one device row
            // cleanly under grayscale AA, matching the ruler-columns
            // rendering convention.
            let x = (margins.left + col_x).floor() + 0.5;
            // Emphasize the deepest guide column on the caret's line.
            let brush = if is_active_line && k == depth - 1 {
                active_color
            } else {
                color
            };
            let rect = D2D_RECT_F {
                left: x,
                top: y,
                right: x + 1.0,
                bottom: y + line_height,
            };
            unsafe { ctx.FillRectangle(&rect, brush) };
        }
    }
}

/// Source line a display row maps to, via the projection's row index.
#[must_use]
fn source_line_for_row(frame_display: &FrameDisplay, display_row: u32) -> Option<usize> {
    frame_display
        .display_line_by_index(display_row)
        .map(|spec| spec.source_line.raw() as usize)
}

/// Boundary x-positions (DIPs from `margins.left`) where each indent
/// unit on `line_idx` ends. A `\t` is one unit of width `tab_advance`;
/// a run of `indent_size` consecutive spaces is one unit of width
/// `indent_size * column_advance`. A trailing partial space run (fewer
/// than `indent_size` spaces) is ignored — it doesn't form a parent
/// level. Returns `None` for a blank line (only whitespace).
fn measure_indent_boundaries(
    rope: &Rope,
    line_idx: usize,
    indent_size: u32,
    column_advance: f32,
    tab_advance: f32,
) -> Option<Vec<f32>> {
    let line = rope.line(line_idx);
    let mut bounds: Vec<f32> = Vec::new();
    let mut x: f32 = 0.0;
    let mut space_run: u32 = 0;
    let mut saw_non_ws = false;
    for ch in line.chars() {
        match ch {
            ' ' => {
                x += column_advance;
                space_run += 1;
                if space_run >= indent_size {
                    bounds.push(x);
                    space_run = 0;
                }
            }
            '\t' => {
                // Tab advances to its own rendered stop. Drop any partial
                // space run; mixed `   \t` indentation collapses to the
                // tab's boundary, matching what the renderer draws.
                if space_run > 0 {
                    x -= space_run as f32 * column_advance;
                    space_run = 0;
                }
                x += tab_advance;
                bounds.push(x);
            }
            '\n' | '\r' => break,
            _ => {
                saw_non_ws = true;
                break;
            }
        }
    }
    if saw_non_ws {
        Some(bounds)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_line_yields_no_boundaries() {
        let rope = Rope::from_str("   \nbody\n");
        // Whitespace-only line ⇒ None (no guides anchored on a blank row).
        assert!(measure_indent_boundaries(&rope, 0, 2, 8.0, 16.0).is_none());
    }

    #[test]
    fn space_run_boundary_at_indent_size_multiples() {
        // 4 leading spaces, indent_size 2, column advance 8 ⇒ two units,
        // boundaries at 16 and 32.
        let rope = Rope::from_str("    code\n");
        let bounds = measure_indent_boundaries(&rope, 0, 2, 8.0, 16.0).expect("non-blank");
        assert_eq!(bounds, vec![16.0, 32.0]);
    }

    #[test]
    fn partial_trailing_space_run_does_not_form_a_level() {
        // 3 spaces with indent_size 2: only the first 2 form a unit (at
        // 16); the dangling third space is not a parent boundary.
        let rope = Rope::from_str("   code\n");
        let bounds = measure_indent_boundaries(&rope, 0, 2, 8.0, 16.0).expect("non-blank");
        assert_eq!(bounds, vec![16.0]);
    }

    #[test]
    fn tab_boundary_uses_tab_advance() {
        let rope = Rope::from_str("\t\tcode\n");
        let bounds = measure_indent_boundaries(&rope, 0, 4, 8.0, 32.0).expect("non-blank");
        assert_eq!(bounds, vec![32.0, 64.0]);
    }

    #[test]
    fn k_zero_guide_is_suppressed() {
        // The painter draws guides for k in 1..depth, so a line at depth 1
        // (one boundary) produces zero painted columns: the body-left-edge
        // guide (k == 0) is intentionally suppressed.
        let rope = Rope::from_str("  code\n");
        let bounds = measure_indent_boundaries(&rope, 0, 2, 8.0, 16.0).expect("non-blank");
        let depth = bounds.len();
        let painted = (1..depth).count();
        assert_eq!(depth, 1);
        assert_eq!(painted, 0);
    }
}
