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

/// Paint vertical indent-guide rules aligned with the first character
/// of each enclosing parent indent level. Tabs and spaces are measured
/// at their actual rendered advance so guides land under the visible
/// leading whitespace boundaries, not under a `space_advance * column`
/// approximation.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_indent_guides(
    ctx: &ID2D1DeviceContext,
    rope: &Rope,
    line_height: f32,
    scroll_y: f32,
    margins: ContentMargins,
    indent_size: u32,
    column_advance: f32,
    tab_advance: f32,
    first_visible: usize,
    last_visible: usize,
    color: &ID2D1SolidColorBrush,
) {
    if indent_size == 0 || column_advance <= 0.0 {
        return;
    }
    let tab_advance = if tab_advance > 0.0 {
        tab_advance
    } else {
        column_advance * indent_size as f32
    };
    let total_lines = rope.len_lines();
    if total_lines == 0 {
        return;
    }
    let last_visible = last_visible.min(total_lines);
    if first_visible >= last_visible {
        return;
    }
    // Phase 17.6 intelligent indent guides: extend the visible window
    // by a small skirt so the blank-line carry-over reaches a non-blank
    // anchor when the user is editing near a paragraph boundary.
    const SKIRT: usize = 64;
    let scan_start = first_visible.saturating_sub(SKIRT);
    let scan_end = (last_visible + SKIRT).min(total_lines);

    // Per-scanned-line indent-unit boundary x-positions in DIPs from
    // `margins.left`. Each entry is the x where one indent unit ends
    // (a `\t` contributes `tab_advance`; a run of `indent_size`
    // consecutive spaces contributes `indent_size * column_advance`).
    // `None` marks a blank line (only whitespace).
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
    // Carry the boundary list of the previous non-blank line forward
    // across blank gaps.
    let mut forward = raw.clone();
    let mut last_bounds: Option<Vec<f32>> = None;
    for slot in forward.iter_mut() {
        match slot {
            Some(v) => last_bounds = Some(v.clone()),
            None => *slot = last_bounds.clone(),
        }
    }
    // Carry the boundary list of the next non-blank line backward; a
    // blank line draws guides up to `min(forward.len(), backward.len())`
    // levels so a gap only spans depths both surrounding paragraphs share.
    let mut backward = raw.clone();
    let mut next_bounds: Option<Vec<f32>> = None;
    for slot in backward.iter_mut().rev() {
        match slot {
            Some(v) => next_bounds = Some(v.clone()),
            None => *slot = next_bounds.clone(),
        }
    }

    for line_idx in first_visible..last_visible {
        let local = line_idx - scan_start;
        // Resolve this line's effective boundary set:
        // - non-blank → its own measured boundaries.
        // - blank with both flanks → use the forward neighbour, truncated
        //   to the shorter flank's length (parents shared on both sides).
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
        let y = line_idx as f32 * line_height - scroll_y;
        // A guide at offset C means "an enclosing parent's content
        // starts at C". For depth N, parents sit at 0, bounds[0], …,
        // bounds[N-2] — i.e. each guide aligns under the first character
        // of the line one indent level shallower, not under this line's
        // own first character.
        for k in 0..depth {
            let col_x = if k == 0 { 0.0 } else { bounds[k - 1] };
            // Half-pixel offset so the 1-DIP rule hits one device row
            // cleanly under DirectWrite's grayscale AA, matching the
            // ruler-columns rendering convention.
            let x = (margins.left + col_x).floor() + 0.5;
            let rect = D2D_RECT_F {
                left: x,
                top: y,
                right: x + 1.0,
                bottom: y + line_height,
            };
            unsafe { ctx.FillRectangle(&rect, color) };
        }
    }
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
