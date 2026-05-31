//! Phase 11 view-toggle drawing: current-line highlight, indent guides,
//! whitespace markers, trailing-whitespace fill, ruler columns,
//! line-number gutter, minimap heatmap, and caret-shape variants.
//!
//! Thread ownership: caller is the UI thread (the only owner of the
//! `ID2D1DeviceContext` and the cached `IDWriteTextLayout`s passed in).

use ropey::Rope;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_POINT_2F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1RenderTarget, ID2D1SolidColorBrush, D2D1_DRAW_TEXT_OPTIONS_NONE,
};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory, IDWriteTextFormat, DWRITE_HIT_TEST_METRICS, DWRITE_TEXT_RANGE,
};

use crate::params::colors::EditorColors;
use crate::params::ViewOptionsDraw;
use crate::Error;

/// Default baseline font size in DIPs used when the dynamic gutter width
/// fn doesn't have a font size to scale from (test harnesses, doc
/// constants). The actual painted gutter width is computed per frame by
/// [`gutter_width_for_font_size`] from the active text format's size.
pub const GUTTER_BASELINE_FONT_SIZE_DIP: f32 = 13.0;

/// Minimum width of the line-number gutter column in DIPs at the
/// baseline font size (~13 DIP). Production paint paths derive their
/// gutter width from [`gutter_width_for_line_count`] instead — the
/// constant remains for tests, layout fallbacks, and any path that does
/// not yet know its active font size or source line count. Sized to fit
/// two digits plus one char of right margin. The editor body starts
/// after this *plus* [`GUTTER_BODY_GAP_DIP`].
pub const GUTTER_WIDTH_DIP: f32 = 22.0;

/// Minimum gutter digit budget. Small buffers still reserve a stable
/// two-digit column so lines 1–9 do not make the body feel pinned to
/// the pane edge.
pub const GUTTER_MIN_DIGITS: u32 = 2;

/// Count decimal digits needed to display the largest visible source
/// line number in a buffer.
#[must_use]
pub fn gutter_digit_count_for_line_count(source_line_count: usize) -> u32 {
    let mut n = source_line_count.max(1);
    let mut digits = 1;
    while n >= 10 {
        n /= 10;
        digits += 1;
    }
    digits.max(GUTTER_MIN_DIGITS)
}

/// Gutter width for the given font size and source line count in DIPs.
/// Sized to fit the largest full line number in the buffer plus one char
/// of right margin so digits do not butt against the gutter↔body
/// divider. Scales linearly with font size so larger UIs get
/// proportionally wider gutters.
#[must_use]
pub fn gutter_width_for_line_count(font_size_dip: f32, source_line_count: usize) -> f32 {
    let font_size_dip = if font_size_dip > 0.0 {
        font_size_dip
    } else {
        GUTTER_BASELINE_FONT_SIZE_DIP
    };
    let digits = gutter_digit_count_for_line_count(source_line_count) as f32;
    let scaled = font_size_dip * 0.55 * (digits + 1.0);
    scaled.max(GUTTER_WIDTH_DIP)
}

/// Gutter width for callers that do not yet know the source line count.
/// Prefer [`gutter_width_for_line_count`] in production geometry.
#[must_use]
pub fn gutter_width_for_font_size(font_size_dip: f32) -> f32 {
    gutter_width_for_line_count(font_size_dip, 99)
}

/// Breathing-room gap between the gutter's right-edge separator and the
/// start of editor text. Without it the leftmost glyph butts up against
/// the separator rule.
pub const GUTTER_BODY_GAP_DIP: f32 = 8.0;

// Re-export the minimap column width from the minimap module so the
// margin resolver and the painter share a single source of truth. The
// previous `chrome::MINIMAP_WIDTH_DIP` heatmap constant moved to
// `crate::minimap` along with the scaled-text rewrite.
pub(crate) use crate::minimap::MINIMAP_WIDTH_DIP;

/// Small constant left padding applied to the editor body when no
/// line-number gutter is showing. Without this the leftmost glyph touches
/// the pane edge and its sidebearing is clipped.
pub const BODY_LEFT_PADDING_DIP: f32 = 8.0;

/// Small constant right padding when no minimap is showing. Keeps the
/// caret at end-of-line off the pane edge and gives soft-wrap a sensible
/// gutter.
pub const BODY_RIGHT_PADDING_DIP: f32 = 8.0;

/// Pre-frame setup the renderer wants from the chrome layer: the editor
/// content's left/right margins after gutter / minimap accounting.
#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct ContentMargins {
    /// Left margin in DIPs (gutter width when line-numbers are on, else
    /// [`BODY_LEFT_PADDING_DIP`]).
    pub left: f32,
    /// Right margin in DIPs (minimap width when on, else
    /// [`BODY_RIGHT_PADDING_DIP`]).
    pub right: f32,
}

impl ContentMargins {
    /// Compute margins using a per-buffer source line count for the
    /// line-number gutter.
    #[must_use]
    pub(crate) fn from_view_options_for_line_count(
        opts: &ViewOptionsDraw<'_>,
        font_size_dip: f32,
        source_line_count: usize,
    ) -> Self {
        let right = resolve_body_right_margin_dip(
            opts.minimap,
            opts.search_minimap_active,
            opts.show_outline_sidebar,
            opts.outline_sidebar_width_dip,
        );
        let left = resolve_body_left_margin_for_line_count_dip(
            opts.line_numbers,
            font_size_dip,
            source_line_count,
        );
        Self { left, right }
    }

    // Phase H2 — `with_centered_body` lives in the sibling
    // `chrome_centered.rs` so the cap on this file stays manageable.
}

/// Left-edge body margin in DIPs. Mirrors the right-edge resolver so the
/// renderer and any UI consumer that has to derive a body-text width
/// (display-map wrap, caret-anchor projection, click hit-tests) agree on
/// where the text column begins.
#[must_use]
pub fn resolve_body_left_margin_dip(line_numbers: bool, font_size_dip: f32) -> f32 {
    resolve_body_left_margin_for_line_count_dip(line_numbers, font_size_dip, 99)
}

/// Left-edge body margin in DIPs for a given buffer line count.
#[must_use]
pub fn resolve_body_left_margin_for_line_count_dip(
    line_numbers: bool,
    font_size_dip: f32,
    source_line_count: usize,
) -> f32 {
    if line_numbers {
        gutter_width_for_line_count(font_size_dip, source_line_count) + GUTTER_BODY_GAP_DIP
    } else {
        BODY_LEFT_PADDING_DIP
    }
}

/// Right-edge body margin in DIPs for a given set of view toggles.
///
/// UI-side consumers (scrollbar hit-test, any future click surface
/// living in the body's right gutter) call this so their geometry
/// agrees with what `ContentMargins::from_view_options` hands the
/// renderer. Single source of truth keeps hit targets from drifting
/// away from painted pixels when a sidebar toggle moves the right
/// edge — without this, toggling the minimap or outline made the
/// scrollbar's hit-test rect sit somewhere to the right of (or
/// underneath) the painted thumb.
///
/// Distraction-free centering further inflates both margins; this
/// helper deliberately ignores it (the scrollbar already accepts the
/// small DF offset on the painted thumb, covered by
/// `HIT_LEFT_SLOP_DIP`).
#[must_use]
pub fn resolve_body_right_margin_dip(
    minimap: bool,
    search_minimap_active: bool,
    show_outline_sidebar: bool,
    outline_sidebar_width_dip: f32,
) -> f32 {
    let mut right = if minimap {
        MINIMAP_WIDTH_DIP
    } else if search_minimap_active {
        crate::search_minimap_paint::SEARCH_MINIMAP_WIDTH_DIP
    } else {
        0.0
    };
    if show_outline_sidebar {
        right += outline_sidebar_width_dip.max(0.0);
    }
    if right == 0.0 {
        right = BODY_RIGHT_PADDING_DIP;
    }
    right
}

/// Paint the current-line highlight rect behind the caret line. Called
/// before per-line text drawing so glyphs draw on top.
///
/// Phase B16: when `display_rows.is_some()` the band spans every
/// display row that belongs to the caret's source line (soft-wrap
/// case). `(first_display_row, row_count)` are 0-indexed against the
/// display projection. Falls back to a single source-line band when
/// `None` (non-wrap path).
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_current_line_highlight(
    ctx: &ID2D1DeviceContext,
    rope: &Rope,
    selections: &[continuity_text::Selection],
    line_height: f32,
    scroll_y: f32,
    viewport_w: f32,
    margins: ContentMargins,
    color: &ID2D1SolidColorBrush,
    display_rows: Option<(u32, u32)>,
) {
    let _ = rope;
    let Some(primary) = selections.first() else {
        return;
    };
    let (first_row, rows) = display_rows.unwrap_or((primary.head.line, 1));
    if rows == 0 {
        return;
    }
    let top = first_row as f32 * line_height - scroll_y;
    let rect = D2D_RECT_F {
        left: margins.left,
        top,
        right: viewport_w - margins.right,
        bottom: top + line_height * rows as f32,
    };
    unsafe { ctx.FillRectangle(&rect, color) };
}

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

/// Paint vertical ruler rules at the columns named in `columns`. Drawn
/// across the full viewport height.
pub(crate) fn paint_ruler_columns(
    ctx: &ID2D1DeviceContext,
    columns: &[u32],
    column_advance: f32,
    margins: ContentMargins,
    viewport_h: f32,
    color: &ID2D1SolidColorBrush,
) {
    if column_advance <= 0.0 {
        return;
    }
    for &col in columns {
        let x = margins.left + (col as f32) * column_advance;
        let rect = D2D_RECT_F {
            left: x,
            top: 0.0,
            right: x + 1.0,
            bottom: viewport_h,
        };
        unsafe { ctx.FillRectangle(&rect, color) };
    }
}

/// Paint a coloured fill on the trailing whitespace runs of every visible
/// line that has any.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_trailing_whitespace(
    ctx: &ID2D1DeviceContext,
    rope: &Rope,
    line_height: f32,
    scroll_y: f32,
    margins: ContentMargins,
    column_advance: f32,
    first_visible: usize,
    last_visible: usize,
    color: &ID2D1SolidColorBrush,
) {
    if column_advance <= 0.0 {
        return;
    }
    let total_lines = rope.len_lines();
    for line_idx in first_visible..last_visible.min(total_lines) {
        let line = rope.line(line_idx);
        let mut trailing: u32 = 0;
        let mut total: u32 = 0;
        let mut saw_nonws = false;
        for ch in line.chars() {
            if ch == '\n' || ch == '\r' {
                break;
            }
            total += 1;
            if matches!(ch, ' ' | '\t') {
                trailing += 1;
            } else {
                saw_nonws = true;
                trailing = 0;
            }
        }
        if !saw_nonws || trailing == 0 {
            continue;
        }
        let y = line_idx as f32 * line_height - scroll_y;
        let start_x = margins.left + (total - trailing) as f32 * column_advance;
        let end_x = margins.left + total as f32 * column_advance;
        let rect = D2D_RECT_F {
            left: start_x,
            top: y,
            right: end_x,
            bottom: y + line_height,
        };
        unsafe { ctx.FillRectangle(&rect, color) };
    }
}

/// Overlay whitespace-marker glyphs (`·` for space, `→` for tab) on every
/// space and tab character of the visible lines.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_whitespace_markers(
    ctx: &ID2D1DeviceContext,
    factory: &IDWriteFactory,
    format: &IDWriteTextFormat,
    rope: &Rope,
    line_height: f32,
    scroll_y: f32,
    margins: ContentMargins,
    column_advance: f32,
    first_visible: usize,
    last_visible: usize,
    color: &ID2D1SolidColorBrush,
) -> Result<(), Error> {
    if column_advance <= 0.0 {
        return Ok(());
    }
    let total_lines = rope.len_lines();
    for line_idx in first_visible..last_visible.min(total_lines) {
        let y = line_idx as f32 * line_height - scroll_y;
        let line = rope.line(line_idx);
        for (col, ch) in (0_u32..).zip(line.chars()) {
            if ch == '\n' || ch == '\r' {
                break;
            }
            let glyph = match ch {
                ' ' => Some('·'),
                '\t' => Some('→'),
                _ => None,
            };
            if let Some(g) = glyph {
                let s: Vec<u16> = std::iter::once(g as u16).collect();
                let layout = unsafe {
                    factory.CreateTextLayout(&s, format, column_advance * 4.0, line_height)?
                };
                let x = margins.left + col as f32 * column_advance;
                unsafe {
                    ctx.DrawTextLayout(
                        D2D_POINT_2F { x, y },
                        &layout,
                        color,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                    );
                }
            }
        }
    }
    Ok(())
}

// Touch a couple of imports so the module's surface is exercised in tests.
const _: fn() = || {
    let _: HWND = HWND::default();
    let _: D2D1_COLOR_F = D2D1_COLOR_F::default();
    let _: DWRITE_HIT_TEST_METRICS = DWRITE_HIT_TEST_METRICS::default();
    let _: DWRITE_TEXT_RANGE = DWRITE_TEXT_RANGE::default();
    let _: fn(&ID2D1RenderTarget) = |_| ();
    let _ = EditorColors::default();
    let _ = MINIMAP_WIDTH_DIP;
};

#[cfg(test)]
mod tests;
