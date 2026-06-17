//! §H3 — fold-triangle painter for the line-number gutter.
//!
//! For every visible source line whose indent subtree has a deeper-
//! indented body, paint a small ▸ / ▾ glyph in the gutter strip. The
//! triangle direction reflects the line's membership in the active
//! `folded_lines` set:
//!
//! - `▾` (down arrow) — line is foldable and **expanded**
//! - `▸` (right arrow) — line is foldable and **folded**
//!
//! Sibling to [`crate::chrome_line_numbers::paint_line_number_gutter`] rather than an
//! extension of it: the gutter painter is already long and the fold
//! triangle has independent input (the folded-lines set), so a separate
//! painter keeps both files under the 600-line cap.
//!
//! **Thread ownership**: UI thread.

use ropey::Rope;
use windows::Win32::Graphics::Direct2D::Common::{D2D_POINT_2F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::D2D1_DRAW_TEXT_OPTIONS_NONE;
use windows::Win32::Graphics::Direct2D::{ID2D1DeviceContext, ID2D1SolidColorBrush};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory, IDWriteTextFormat, DWRITE_TEXT_ALIGNMENT_CENTER,
};

use crate::Error;

/// Glyph used when a line is foldable and currently expanded.
const GLYPH_EXPANDED: char = '▾';
/// Glyph used when a line is foldable and currently folded.
const GLYPH_FOLDED: char = '▸';

/// §10 — whether a foldable line's tick should paint this frame.
/// Collapsed lines always show their `▸` tick (so a folded section stays
/// discoverable); expanded lines only show their `▾` tick while the
/// pointer is over the gutter strip. Pure so the gating is unit-testable
/// without a D2D device.
#[must_use]
fn should_paint_fold_tick(is_folded: bool, gutter_hovered: bool) -> bool {
    is_folded || gutter_hovered
}

/// Header + body span of one active fold, used by the gutter painter
/// to skip body-line numbers and stamp the "▸ N" indicator on the
/// header line.
///
/// `body_line_count` is `end_line_exclusive - header_line - 1` (so a
/// fold whose subtree spans lines 3..=6 has `body_line_count == 3`).
/// Header-only "folds" (no deeper body) are not produced by
/// [`compute_fold_headers`] — they have nothing to hide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoldHeaderInfo {
    /// Source-line index of the fold's header (the unfolded line that
    /// stays visible).
    pub header_line: u32,
    /// Exclusive end of the fold's source-line range.
    pub end_line_exclusive: u32,
    /// Body line count — the number of hidden source lines below the
    /// header. Equivalently, `end_line_exclusive - header_line - 1`.
    pub body_line_count: u32,
}

/// Translate the user-toggled `folded_lines` set into a list of
/// `(header, end_exclusive, body_count)` entries the gutter painter
/// can use to skip body-line numbers and stamp "▸ N" on header rows.
///
/// Two fold kinds are unified here:
/// - **Indent folds** — a line whose indent subtree has a deeper body.
///   Geometry computed via the local indent-subtree algorithm.
/// - **Heading folds** — a markdown heading line. Geometry runs to the
///   next heading of the same or shallower level (or EOF), using the
///   supplied `headings` slice `(line, level)`.
///
/// Sentinel: `u32::MAX` expands to every column-0 indent line whose
/// subtree has a body **and** every heading at the shallowest level
/// present (H1 if any, else the smallest `level` number that appears).
///
/// On conflict (a line is both an indent header and a heading), the
/// **heading** subtree wins because it is usually larger and the
/// caller's coalesce step would extend the merged range to the larger
/// end anyway. Implemented by emitting indent first, then heading; the
/// later push overrides via a same-`header_line` replace.
///
/// The returned vec is sorted ascending by `header_line` with no
/// duplicate headers, so the gutter painter's lookup (linear scan)
/// stays O(visible_rows).
#[must_use]
pub fn compute_fold_headers(
    rope: &Rope,
    folded_lines: &[u32],
    headings: &[(u32, u8)],
) -> Vec<FoldHeaderInfo> {
    let total = rope.len_lines();
    let mut out: Vec<FoldHeaderInfo> = Vec::new();
    let push_header = |out: &mut Vec<FoldHeaderInfo>, header: u32| {
        let h = header as usize;
        if h >= total {
            return;
        }
        let end = indent_subtree_end_line_exclusive(rope, h);
        if end <= h + 1 {
            return; // single-line subtree — nothing to indicate.
        }
        let info = FoldHeaderInfo {
            header_line: header,
            end_line_exclusive: end as u32,
            body_line_count: (end - h - 1) as u32,
        };
        if !out.iter().any(|e| e.header_line == header) {
            out.push(info);
        }
    };
    // Heading-fold variant of `push_header`: end line comes from the
    // next heading at the same-or-shallower level (or EOF). When a
    // line is both an indent header and a heading, the heading entry
    // replaces the indent entry in the output (headings usually fold
    // more lines, matching the merge step in `window_paint`).
    let push_heading = |out: &mut Vec<FoldHeaderInfo>, header: u32, level: u8| {
        let h = header as usize;
        if h >= total {
            return;
        }
        let next_idx = headings.iter().position(|&(hl, _)| hl == header);
        let end_line = match next_idx {
            Some(idx) => headings[idx + 1..]
                .iter()
                .find(|(_, l)| *l <= level)
                .map(|(line, _)| *line)
                .unwrap_or(total as u32),
            None => return,
        };
        if end_line as usize <= h + 1 {
            return; // empty body — nothing to indicate.
        }
        let info = FoldHeaderInfo {
            header_line: header,
            end_line_exclusive: end_line,
            body_line_count: end_line.saturating_sub(header + 1),
        };
        if let Some(slot) = out.iter_mut().find(|e| e.header_line == header) {
            *slot = info; // heading wins over any indent entry at this line.
        } else {
            out.push(info);
        }
    };

    let fold_all = folded_lines.contains(&u32::MAX);
    if fold_all {
        for line_idx in 0..total {
            if line_indent_columns(rope, line_idx) == 0 {
                let line_u32 = match u32::try_from(line_idx) {
                    Ok(n) => n,
                    Err(_) => continue,
                };
                push_header(&mut out, line_u32);
            }
        }
        // §H3 heading sentinel: every heading at the shallowest level.
        if let Some(min_level) = headings.iter().map(|(_, l)| *l).min() {
            for &(line, level) in headings.iter().filter(|(_, l)| *l == min_level) {
                push_heading(&mut out, line, level);
            }
        }
    }
    for &line in folded_lines {
        if line == u32::MAX {
            continue;
        }
        push_header(&mut out, line);
        if let Some(&(_, level)) = headings.iter().find(|(hl, _)| *hl == line) {
            push_heading(&mut out, line, level);
        }
    }
    out.sort_unstable_by_key(|e| e.header_line);
    out
}

/// Compute the exclusive end line of the indent subtree starting at
/// `line`. Mirrors the algorithm in
/// `continuity_core::edit_indent_subtree::indent_subtree`, kept local
/// to `render` because the layer graph forbids `render → core`.
fn indent_subtree_end_line_exclusive(rope: &Rope, line: usize) -> usize {
    let total = rope.len_lines();
    if line >= total {
        return line;
    }
    let base = line_indent_columns(rope, line);
    if base == u32::MAX {
        return line + 1;
    }
    let mut end = line + 1;
    while end < total {
        let i = line_indent_columns(rope, end);
        if i == u32::MAX {
            // Blank line — peek forward; absorb when the next non-blank
            // line is deeper than base, else stop.
            let mut peek = end + 1;
            while peek < total && line_indent_columns(rope, peek) == u32::MAX {
                peek += 1;
            }
            if peek < total && line_indent_columns(rope, peek) > base {
                end = peek + 1;
                continue;
            }
            break;
        }
        if i <= base {
            break;
        }
        end += 1;
    }
    end
}

/// Returns `true` when `line` is foldable — either because its indent
/// subtree has a deeper-indented body, or (when `heading_lines`
/// contains `line as u32`) because the line is a markdown heading.
/// Blank-line peeking still applies to the indent case.
///
/// Pure function over `&Rope` so this module can stay independent of
/// `continuity_core` (the layer graph forbids `render → core`).
#[must_use]
pub(crate) fn is_line_foldable_with_headings(
    rope: &Rope,
    line: usize,
    heading_lines: &[u32],
) -> bool {
    if let Ok(line_u32) = u32::try_from(line) {
        if heading_lines.contains(&line_u32) {
            return true;
        }
    }
    is_line_foldable(rope, line)
}

/// Indent-only foldability check. Kept for the in-module tests; the
/// painter uses [`is_line_foldable_with_headings`] which delegates
/// here when the line isn't a heading.
#[must_use]
pub(crate) fn is_line_foldable(rope: &Rope, line: usize) -> bool {
    let total = rope.len_lines();
    if line >= total {
        return false;
    }
    let base = line_indent_columns(rope, line);
    if base == u32::MAX {
        return false;
    }
    let mut probe = line + 1;
    while probe < total {
        let next = line_indent_columns(rope, probe);
        if next == u32::MAX {
            probe += 1;
            continue;
        }
        return next > base;
    }
    false
}

/// Leading-indent column count for `line`, matching the convention of
/// `continuity_core::edit_indent_subtree::line_indent`: spaces count as 1
/// each, tabs as 4, blank lines return `u32::MAX`.
fn line_indent_columns(rope: &Rope, line: usize) -> u32 {
    let total = rope.len_lines();
    if line >= total {
        return 0;
    }
    let start = rope.line_to_byte(line);
    let end = if line + 1 < total {
        rope.line_to_byte(line + 1)
    } else {
        rope.len_bytes()
    };
    let slice = rope.byte_slice(start..end);
    let mut indent = 0u32;
    let mut any_non_ws = false;
    for ch in slice.chars() {
        match ch {
            ' ' => indent += 1,
            '\t' => indent += 4,
            '\r' | '\n' => break,
            _ => {
                any_non_ws = true;
                break;
            }
        }
    }
    if any_non_ws {
        indent
    } else {
        u32::MAX
    }
}

/// Paint fold triangles in the gutter strip's left column for every
/// visible foldable source line. The triangle reflects fold state from
/// `folded_lines`.
///
/// `folded_lines` is the user-toggled set from
/// `continuity_ui::window_pane_modes::PaneModesState`. A `u32::MAX`
/// sentinel in the set indicates "fold all top-level"; for the painter's
/// purpose it is interpreted as "every column-0 line is folded".
///
/// `brush` paints the expanded `▾` glyph (muted, matching the gutter
/// numbers); `folded_brush` paints the collapsed `▸` glyph so a folded
/// section's caret stands out a little from the rest of the gutter.
///
/// `gutter_hovered` gates the **expanded** `▾` ticks: they only paint
/// when the pointer is over the gutter strip, so an idle gutter stays
/// quiet (it doesn't decorate every foldable line with a glyph). The
/// **folded** `▸` ticks always paint regardless of hover — a collapsed
/// section must advertise that it can be re-expanded even when the
/// pointer is elsewhere.
///
/// # Errors
///
/// Returns [`Error::Graphics`] when DirectWrite text-layout allocation
/// fails for any glyph.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_fold_triangles(
    ctx: &ID2D1DeviceContext,
    factory: &IDWriteFactory,
    format: &IDWriteTextFormat,
    rope: &Rope,
    folded_lines: &[u32],
    heading_lines: &[u32],
    line_height: f32,
    scroll_y: f32,
    first_visible: usize,
    last_visible: usize,
    brush: &ID2D1SolidColorBrush,
    folded_brush: &ID2D1SolidColorBrush,
    gutter_hovered: bool,
) -> Result<(), Error> {
    let total_lines = rope.len_lines();
    let fold_all = folded_lines.contains(&u32::MAX);
    // When the gutter is not hovered and nothing is collapsed there is
    // nothing to draw: expanded ticks are hover-gated and there are no
    // folded sections to advertise. Bail before any per-line work.
    if !gutter_hovered && folded_lines.is_empty() {
        return Ok(());
    }
    let font_size_dip = unsafe { format.GetFontSize() };
    let gutter_width = crate::chrome::gutter_width_for_line_count(font_size_dip, rope.len_lines());
    // The fold icons live inside the gutter's right-edge gap — the same
    // gap the line-number digits are inset from — so digits and icons
    // never overlap, at any font size or buffer line count.
    let fold_gap = crate::chrome::gutter_fold_gap_dip(font_size_dip);
    let triangle_x = (gutter_width - fold_gap).max(0.0);
    let triangle_rect_left = triangle_x;
    // Paint a faint background panel so the triangle column reads as a
    // distinct gutter sub-region. Caller (chrome_post) is responsible
    // for setting the transform; we draw at the absolute gutter strip.
    let _ = triangle_rect_left; // panel is intentionally chrome-less to start.

    for line_idx in first_visible..last_visible.min(total_lines) {
        if !is_line_foldable_with_headings(rope, line_idx, heading_lines) {
            continue;
        }
        let line_u32 = match u32::try_from(line_idx) {
            Ok(n) => n,
            Err(_) => continue,
        };
        let is_folded = folded_lines.contains(&line_u32)
            || (fold_all && line_indent_columns(rope, line_idx) == 0);
        // Expanded `▾` ticks only paint while the gutter is hovered; the
        // folded `▸` tick always paints so a collapsed section stays
        // discoverable. Skip expanded ticks on an un-hovered gutter.
        if !should_paint_fold_tick(is_folded, gutter_hovered) {
            continue;
        }
        let (glyph, glyph_brush) = if is_folded {
            // A collapsed section's caret is tinted with the brighter
            // active-line gutter color so it stands out from the muted
            // expanded carets around it.
            (GLYPH_FOLDED, folded_brush)
        } else {
            (GLYPH_EXPANDED, brush)
        };
        let y = line_idx as f32 * line_height - scroll_y;
        let wide: Vec<u16> = std::iter::once(glyph as u16).collect();
        let layout = unsafe { factory.CreateTextLayout(&wide, format, fold_gap, line_height)? };
        // Center the glyph within the fold gap so it sits midway between
        // the last digit and the gutter↔body divider, scaling with the
        // gap (and therefore the font size).
        unsafe {
            let _ = layout.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
        }
        unsafe {
            ctx.DrawTextLayout(
                D2D_POINT_2F { x: triangle_x, y },
                &layout,
                glyph_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }
        // Reserve the rect for hover/click hit-testing landed in a follow-up.
        let _hit_rect = D2D_RECT_F {
            left: triangle_x,
            top: y,
            right: triangle_x + fold_gap,
            bottom: y + line_height,
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;

    #[test]
    fn line_with_deeper_body_is_foldable() {
        let rope = Rope::from_str("parent\n  child\nnext\n");
        assert!(is_line_foldable(&rope, 0));
        assert!(!is_line_foldable(&rope, 1));
        assert!(!is_line_foldable(&rope, 2));
    }

    #[test]
    fn blank_line_is_not_foldable() {
        let rope = Rope::from_str("\n  body\n");
        // Line 0 is blank → indent_columns is u32::MAX → not foldable.
        assert!(!is_line_foldable(&rope, 0));
    }

    #[test]
    fn blank_intermediate_line_is_skipped_when_probing() {
        let rope = Rope::from_str("parent\n\n  body\n");
        // Line 0 has indent 0; blank line 1 is skipped; line 2 has indent 2 → deeper.
        assert!(is_line_foldable(&rope, 0));
    }

    #[test]
    fn deepest_line_in_subtree_is_not_foldable() {
        let rope = Rope::from_str("a\n  b\n    c\n");
        // Line 2 has the deepest indent — nothing deeper follows.
        assert!(!is_line_foldable(&rope, 2));
    }

    #[test]
    fn out_of_range_line_is_not_foldable() {
        let rope = Rope::from_str("only\n");
        assert!(!is_line_foldable(&rope, 99));
    }

    #[test]
    fn compute_fold_headers_empty_when_no_folds() {
        let rope = Rope::from_str("parent\n  child\n");
        assert!(compute_fold_headers(&rope, &[], &[]).is_empty());
    }

    #[test]
    fn compute_fold_headers_yields_header_with_body_count() {
        let rope = Rope::from_str("parent\n  child\n  child2\nsibling\n");
        let headers = compute_fold_headers(&rope, &[0], &[]);
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].header_line, 0);
        assert_eq!(headers[0].end_line_exclusive, 3);
        assert_eq!(headers[0].body_line_count, 2);
    }

    #[test]
    fn compute_fold_headers_skips_single_line_subtrees() {
        let rope = Rope::from_str("alpha\nbeta\ngamma\n");
        // None of these have a deeper body — header list should be empty.
        let headers = compute_fold_headers(&rope, &[0, 1, 2], &[]);
        assert!(headers.is_empty());
    }

    #[test]
    fn compute_fold_headers_drops_indices_past_eof() {
        let rope = Rope::from_str("foo\n");
        assert!(compute_fold_headers(&rope, &[42], &[]).is_empty());
    }

    #[test]
    fn compute_fold_headers_sentinel_expands_to_top_level() {
        let rope = Rope::from_str("alpha\n  a1\n  a2\nbeta\n  b1\n");
        let headers = compute_fold_headers(&rope, &[u32::MAX], &[]);
        assert_eq!(headers.len(), 2);
        assert_eq!(headers[0].header_line, 0);
        assert_eq!(headers[0].body_line_count, 2);
        assert_eq!(headers[1].header_line, 3);
        assert_eq!(headers[1].body_line_count, 1);
    }

    #[test]
    fn compute_fold_headers_sorted_and_deduplicated() {
        let rope = Rope::from_str("parent\n  child\nsibling\n  child2\n");
        // Both 0 and 2 are valid headers; provider de-duplicates against
        // multiple entries and sorts ascending.
        let headers = compute_fold_headers(&rope, &[2, 0, 0], &[]);
        assert_eq!(headers.len(), 2);
        assert_eq!(headers[0].header_line, 0);
        assert_eq!(headers[1].header_line, 2);
    }

    #[test]
    fn compute_fold_headers_includes_heading_fold() {
        // "# H1\nbody\n## H2\nbody2\n" — len_lines is 5 (ropey
        // counts the empty after a trailing \n). Fold H2 at line 2.
        let rope = Rope::from_str("# H1\nbody\n## H2\nbody2\n");
        let headings = vec![(0u32, 1u8), (2, 2)];
        let headers = compute_fold_headers(&rope, &[2], &headings);
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].header_line, 2);
        // H2 fold extends to EOF; ropey counts an empty line past the
        // trailing \n, so end_line_exclusive = len_lines = 5.
        assert_eq!(headers[0].end_line_exclusive, 5);
        assert_eq!(headers[0].body_line_count, 2);
    }

    #[test]
    fn compute_fold_headers_heading_wins_over_indent() {
        // Line 0 is both an indent header (has indented body) AND a
        // heading. Heading fold extends further, so heading entry
        // replaces the indent entry.
        let rope = Rope::from_str("# H1\n  body\n# H2\nbody2\n");
        let headings = vec![(0u32, 1u8), (2, 1)];
        let headers = compute_fold_headers(&rope, &[0], &headings);
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].header_line, 0);
        // Heading-fold end = line 2 (next H1).
        assert_eq!(headers[0].end_line_exclusive, 2);
    }

    #[test]
    fn is_line_foldable_with_headings_recognizes_heading() {
        let rope = Rope::from_str("# H1\nbody\n");
        // Line 0 is a heading; no indent body. With headings list,
        // foldable. Without it, not foldable.
        assert!(!is_line_foldable(&rope, 0));
        assert!(is_line_foldable_with_headings(&rope, 0, &[0]));
    }

    #[test]
    fn sentinel_expands_to_both_top_level_indents_and_headings() {
        // Two top-level indent subtrees (alpha, beta) AND a heading.
        // Sentinel should produce entries for both kinds.
        let rope = Rope::from_str("alpha\n  a1\n  a2\nbeta\n  b1\n# H1\nbody\n# H2\nbody2\n");
        let headings = vec![(5u32, 1u8), (7, 1)];
        let headers = compute_fold_headers(&rope, &[u32::MAX], &headings);
        // Expect entries for indent headers (lines 0, 3) AND heading
        // headers (lines 5, 7).
        let header_lines: Vec<u32> = headers.iter().map(|h| h.header_line).collect();
        assert!(header_lines.contains(&0));
        assert!(header_lines.contains(&3));
        assert!(header_lines.contains(&5));
        assert!(header_lines.contains(&7));
    }

    #[test]
    fn line_indent_counts_spaces_and_tabs() {
        let rope = Rope::from_str("  a\n\tb\nc\n");
        assert_eq!(line_indent_columns(&rope, 0), 2);
        assert_eq!(line_indent_columns(&rope, 1), 4);
        assert_eq!(line_indent_columns(&rope, 2), 0);
    }

    #[test]
    fn expanded_tick_only_paints_when_gutter_hovered() {
        // Expanded (not folded): hidden when the gutter is idle, shown on hover.
        assert!(!should_paint_fold_tick(false, false));
        assert!(should_paint_fold_tick(false, true));
    }

    #[test]
    fn folded_tick_always_paints() {
        // Collapsed: visible regardless of hover so it stays discoverable.
        assert!(should_paint_fold_tick(true, false));
        assert!(should_paint_fold_tick(true, true));
    }
}
