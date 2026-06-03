//! Block-level D2D draw helpers: blockquote left bar, code-block panel,
//! horizontal rule. Called from `Renderer::draw_buffer`.
//!
//! Hidden-marker overlay rectangles and bullet glyph substitution moved
//! into the display map (Phase 17.6): markers are simply *omitted* from
//! the display string, and bullet substitution lives in
//! `display_map::builder` as a `Replace` segment. The renderer never
//! paints over source glyphs to fake reveal/hide anymore.
//!
//! **Thread ownership**: caller is the UI thread (the only owner of the
//! `ID2D1DeviceContext` and the cached `IDWriteTextLayout`s).

use continuity_decorate::{BlockKind, Decorations};
use continuity_text::Selection;
use ropey::Rope;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{ID2D1DeviceContext, ID2D1SolidColorBrush};

use crate::display_projection::FrameDisplay;
use crate::Rgba;

/// Paint background panels for fenced/indented code blocks and for
/// blockquotes (left vertical bar). Called *before* the per-line text so
/// the panel sits behind glyphs.
///
/// Top/bottom edges are translated through `frame_display` so a block
/// whose source lines sit below wrapped or folded content aligns with
/// the glyphs (which paint in display-row space) instead of drifting
/// up the viewport.
///
/// Fenced-block panels clip their right edge to the actual content
/// width (longest content line × `column_advance`, plus breathing
/// margin) so a short code block doesn't paint a full-viewport-wide
/// band — the user-visible result of the un-clipped behaviour was a
/// 10-character snippet occupying a horizon-wide highlight.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_block_backgrounds(
    ctx: &ID2D1DeviceContext,
    rope: &Rope,
    frame_display: &FrameDisplay,
    decorations: &Decorations,
    code_panel_brush: &ID2D1SolidColorBrush,
    code_panel_header_brush: &ID2D1SolidColorBrush,
    blockquote_bar_brush: &ID2D1SolidColorBrush,
    line_height: f32,
    viewport_width_dip: f32,
    scroll_y_dip: f32,
    column_advance: f32,
    body_text_left_dip: f32,
) {
    for block in &decorations.blocks {
        let first = byte_to_line(rope, block.start_byte);
        let last = byte_to_line(rope, block.end_byte.saturating_sub(1));
        let Some((top, bottom)) =
            block_display_span(frame_display, first, last, line_height, scroll_y_dip)
        else {
            continue;
        };
        match block.kind {
            BlockKind::FencedCodeBlock => {
                // Content width is measured from `body_text_left_dip`
                // (i.e. the first text column), so a fenced block
                // hugs the glyphs instead of bleeding into the
                // line-number gutter.
                let inner_width = fenced_block_right_edge(
                    rope,
                    first,
                    last,
                    column_advance,
                    (viewport_width_dip - body_text_left_dip).max(0.0),
                );
                let left = fenced_block_left_edge(body_text_left_dip);
                let right = body_text_left_dip + inner_width;
                let rect = D2D_RECT_F {
                    left,
                    top,
                    right,
                    bottom,
                };
                unsafe { ctx.FillRectangle(&rect, code_panel_brush) };
                let header_bottom = (top + line_height).min(bottom);
                if header_bottom > top {
                    let header_rect = D2D_RECT_F {
                        left,
                        top,
                        right,
                        bottom: header_bottom,
                    };
                    unsafe { ctx.FillRectangle(&header_rect, code_panel_header_brush) };
                }
            }
            // Indented code blocks (CommonMark's 4-space-prefix rule)
            // are an implicit classification: paragraphs separated from
            // a list by a blank line and indented 4+ spaces get bucketed
            // as code. Painting that as a code panel is intrusive in a
            // notes editor — the user typically meant "still indented
            // prose", not "this is code". Fenced blocks (` ``` `) stay
            // highlighted because they're an explicit opt-in.
            BlockKind::IndentedCodeBlock => {}
            BlockKind::BlockQuote => {
                let rect = D2D_RECT_F {
                    left: 0.0,
                    top,
                    right: 3.0,
                    bottom,
                };
                unsafe { ctx.FillRectangle(&rect, blockquote_bar_brush) };
            }
            _ => {}
        }
    }
}

/// Paint horizontal rule lines for any HR block. Call after text drawing
/// so the rule overlays the source `---` glyphs.
///
/// `body_left_dip` is the left edge of the text-body column in the
/// current transform (i.e. `margins.left` under `body_translate`). The
/// rule is confined to `[body_left_dip + INSET, body_left_dip +
/// body_width_dip - INSET]` so it doesn't extend into the line-number
/// gutter when one is active.
///
/// Lines that contain a caret (any selection head's source line)
/// are skipped so the user sees the raw `---` source while editing —
/// matching how marker reveal works for headings / bullets / tables.
///
/// `frame_display` maps the HR's source line to a display row so soft-
/// wrapped or folded content above the rule shifts the painted Y by the
/// same amount as the glyphs. Without this translation an HR low in a
/// soft-wrapped buffer paints on top of whatever line happens to sit at
/// `source_line * line_height` — the false-divider symptom.
/// Focused-pane thematic-break paint pass. Wraps [`paint_horizontal_rules`]
/// with the decorations-present guard, the `[markdown].render_divider`
/// read, and the timing capture so the renderer's main draw routine stays
/// under the 600-line cap. Returns the microseconds spent (0 when there
/// are no decorations to paint against). Owning thread: the renderer's UI
/// thread.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_horizontal_rules_pass(
    ctx: &ID2D1DeviceContext,
    rope: &Rope,
    selections: &[Selection],
    params: &crate::params::DrawParams<'_>,
    rule_brush: &ID2D1SolidColorBrush,
    line_height: f32,
    body_left_dip: f32,
    body_width_dip: f32,
    scroll_y_dip: f32,
) -> u64 {
    let Some(decorations) = params.decorations else {
        return 0;
    };
    let started = std::time::Instant::now();
    paint_horizontal_rules(
        ctx,
        rope,
        params.frame_display,
        decorations,
        selections,
        rule_brush,
        line_height,
        body_left_dip,
        body_width_dip,
        scroll_y_dip,
        params.view_options.render_divider,
    );
    started.elapsed().as_micros() as u64
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_horizontal_rules(
    ctx: &ID2D1DeviceContext,
    rope: &Rope,
    frame_display: &FrameDisplay,
    decorations: &Decorations,
    selections: &[Selection],
    rule_brush: &ID2D1SolidColorBrush,
    line_height: f32,
    body_left_dip: f32,
    body_width_dip: f32,
    scroll_y_dip: f32,
    render_divider: bool,
) {
    // `render_divider` OFF (`[markdown].render_divider = false`) keeps the
    // literal `---` / `***` / `___` characters visible (the display map
    // does not hide them) and skips the rule paint entirely. Gating here
    // covers both the focused-pane and spectator-pane call sites without
    // an `if` branch at either.
    if !render_divider {
        return;
    }
    const INSET_DIP: f32 = 8.0;
    let left = body_left_dip + INSET_DIP;
    let right = (body_left_dip + body_width_dip - INSET_DIP).max(left);
    for block in &decorations.blocks {
        if !matches!(block.kind, BlockKind::HorizontalRule) {
            continue;
        }
        let source_line = byte_to_line(rope, block.start_byte);
        if selections
            .iter()
            .any(|s| s.head.line as usize == source_line)
        {
            continue;
        }
        let Some(display_row) = visible_display_row(frame_display, source_line) else {
            continue;
        };
        let y = display_row as f32 * line_height - scroll_y_dip + line_height * 0.5;
        let rect = D2D_RECT_F {
            left,
            top: y - 0.75,
            right,
            bottom: y + 0.75,
        };
        unsafe { ctx.FillRectangle(&rect, rule_brush) };
    }
}

/// First on-screen display-row index for `source_line`. Returns `None`
/// when the source line has no display rows (folded), so the caller
/// skips painting rather than landing on the next visible line.
fn visible_display_row(frame_display: &FrameDisplay, source_line: usize) -> Option<u32> {
    if frame_display.display_line_count_for_source(source_line) == 0 {
        return None;
    }
    Some(frame_display.first_display_line_index_for_source(source_line))
}

/// Top/bottom viewport-local DIPs for the block whose source-line range
/// is `[first_source_line, last_source_line]` (inclusive). Bottom is the
/// exclusive edge of the last visible source line's display rows so a
/// multi-line code block panel covers every wrap segment inside the
/// block. Returns `None` when every line in the range is folded.
fn block_display_span(
    frame_display: &FrameDisplay,
    first_source_line: usize,
    last_source_line: usize,
    line_height: f32,
    scroll_y_dip: f32,
) -> Option<(f32, f32)> {
    let top_row = visible_display_row(frame_display, first_source_line)?;
    let mut cursor = last_source_line;
    let bottom_row = loop {
        let count = frame_display.display_line_count_for_source(cursor);
        if count > 0 {
            let first = frame_display.first_display_line_index_for_source(cursor);
            break first + count;
        }
        if cursor == first_source_line {
            return None;
        }
        cursor -= 1;
    };
    let top = top_row as f32 * line_height - scroll_y_dip;
    let bottom = bottom_row as f32 * line_height - scroll_y_dip;
    Some((top, bottom))
}

fn byte_to_line(rope: &Rope, byte: usize) -> usize {
    let clamped = byte.min(rope.len_bytes());
    rope.byte_to_line(clamped)
}

/// Horizontal padding (DIPs) added to the right of a fenced block's
/// longest content line so the highlight has breathing room beyond the last
/// glyph. `pub` so the UI's copy-button hit-test can match the
/// painter exactly.
pub const FENCED_BLOCK_RIGHT_PADDING_DIP: f32 = 28.0;

/// Horizontal padding (DIPs) added to the left of a fenced block's
/// highlight so left-bearing glyphs do not visually escape the panel.
/// `pub` so the UI copy-button hit-test can match the painter exactly.
pub const FENCED_BLOCK_LEFT_PADDING_DIP: f32 = 4.0;

/// Safety factor over the renderer's column advance. The base advance
/// is a heuristic in both render and UI; a modest scale keeps wide
/// proportional glyphs from escaping the panel while still avoiding
/// full-width blocks for ordinary snippets.
const FENCED_BLOCK_WIDTH_ADVANCE_SCALE: f32 = 1.18;

/// Compute the left edge (in body-local DIPs) of a fenced code block's
/// background highlight from the first body-text column.
#[must_use]
pub fn fenced_block_left_edge(body_text_left_dip: f32) -> f32 {
    (body_text_left_dip - FENCED_BLOCK_LEFT_PADDING_DIP).max(0.0)
}

/// Derive the subtle header-row color for fenced code blocks from the
/// existing block background theme color.
#[must_use]
pub(crate) fn compute_code_block_header_color(base: Rgba) -> Rgba {
    Rgba {
        r: base.r * 0.9,
        g: base.g * 0.9,
        b: base.b * 0.9,
        a: (base.a + 0.04).min(1.0),
    }
}

/// Compute the right edge (in body-local DIPs) of a fenced code
/// block's background highlight given its source-line range and the
/// active `column_advance`. The returned edge equals
/// `longest_content_line_columns * column_advance + FENCED_BLOCK_RIGHT_PADDING_DIP`
/// clamped to `viewport_width_dip`.
///
/// `first_source_line` / `last_source_line` are inclusive. The opening
/// fence line and a fence-only closing line are ignored so panel width
/// follows the code content, not the markdown markers.
#[must_use]
pub fn fenced_block_right_edge(
    rope: &Rope,
    first_source_line: usize,
    last_source_line: usize,
    column_advance: f32,
    viewport_width_dip: f32,
) -> f32 {
    let first_content_line = first_source_line.saturating_add(1);
    let last_content_line =
        if last_source_line > first_source_line && is_fence_only_line(rope, last_source_line) {
            last_source_line.saturating_sub(1)
        } else {
            last_source_line
        };
    if first_content_line > last_content_line {
        return FENCED_BLOCK_RIGHT_PADDING_DIP.min(viewport_width_dip.max(0.0));
    }
    let mut max_columns: usize = 0;
    let total_lines = rope.len_lines();
    let mut line_idx = first_content_line;
    while line_idx <= last_content_line && line_idx < total_lines {
        let columns = line_visual_columns(rope, line_idx);
        if columns > max_columns {
            max_columns = columns;
        }
        line_idx += 1;
    }
    let advance = column_advance.max(1.0) * FENCED_BLOCK_WIDTH_ADVANCE_SCALE;
    let content_width = max_columns as f32 * advance;
    let with_padding = content_width + FENCED_BLOCK_RIGHT_PADDING_DIP;
    with_padding
        .max(FENCED_BLOCK_RIGHT_PADDING_DIP)
        .min(viewport_width_dip.max(0.0))
}

fn line_visual_columns(rope: &Rope, source_line: usize) -> usize {
    let slice = rope.line(source_line);
    let mut columns = 0usize;
    let mut end_byte = slice.len_bytes();
    while end_byte > 0 {
        let b = slice.byte(end_byte - 1);
        if b == b'\n' || b == b'\r' {
            end_byte -= 1;
        } else {
            break;
        }
    }
    for ch in slice.byte_slice(0..end_byte).chars() {
        columns = columns.saturating_add(if ch == '\t' { 4 } else { 1 });
    }
    columns
}

fn is_fence_only_line(rope: &Rope, source_line: usize) -> bool {
    if source_line >= rope.len_lines() {
        return false;
    }
    let slice = rope.line(source_line);
    let mut fence_count = 0usize;
    let mut idx = 0usize;
    let mut end = slice.len_bytes();
    while end > 0 {
        let b = slice.byte(end - 1);
        if matches!(b, b'\n' | b'\r' | b' ' | b'\t') {
            end -= 1;
        } else {
            break;
        }
    }
    while idx < end {
        let b = slice.byte(idx);
        if matches!(b, b' ' | b'\t') {
            idx += 1;
            continue;
        }
        if matches!(b, b'`' | b'~') {
            fence_count += 1;
            idx += 1;
            continue;
        }
        return false;
    }
    fence_count >= 3
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_wrap(rope: &Rope) -> FrameDisplay {
        FrameDisplay::build(rope, 1, None, &[], 0, 8.0)
    }

    #[test]
    fn visible_display_row_no_wrap_matches_source_line() {
        let rope = Rope::from_str("a\nb\nc\nd\n");
        let fd = no_wrap(&rope);
        assert_eq!(visible_display_row(&fd, 0), Some(0));
        assert_eq!(visible_display_row(&fd, 2), Some(2));
        assert_eq!(visible_display_row(&fd, 3), Some(3));
    }

    #[test]
    fn block_display_span_no_wrap_one_row_per_source_line() {
        let rope = Rope::from_str("a\nb\nc\nd\ne\n");
        let fd = no_wrap(&rope);
        let (top, bottom) =
            block_display_span(&fd, 1, 3, 20.0, 0.0).expect("unfolded range must have a span");
        // First display row of source 1 is 1; last source 3 occupies
        // one display row at index 3 → bottom row 4. 4 * 20 = 80.
        assert!((top - 20.0).abs() < 1e-3, "top={top}");
        assert!((bottom - 80.0).abs() < 1e-3, "bottom={bottom}");
    }

    #[test]
    fn block_display_span_applies_scroll_offset() {
        let rope = Rope::from_str("a\nb\nc\n");
        let fd = no_wrap(&rope);
        let (top, bottom) = block_display_span(&fd, 0, 2, 16.0, 8.0).unwrap();
        // top row 0 → -8, bottom row 3 → 48 - 8 = 40.
        assert!((top + 8.0).abs() < 1e-3, "top={top}");
        assert!((bottom - 40.0).abs() < 1e-3, "bottom={bottom}");
    }

    #[test]
    fn soft_wrap_above_shifts_hr_into_display_row_space() {
        // A long first line forces multiple wrap segments before the
        // thematic-break line. Source line 1 (`---`) lands at a display
        // row strictly greater than 1, which is the false-divider
        // scenario: the old code painted at `1 * line_height`, on top of
        // the wrapped content of line 0.
        let rope = Rope::from_str("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n---\n");
        // wrap_width 16 dip with 8 dip/char ⇒ ~2 chars per wrap row.
        let fd = FrameDisplay::build(&rope, 1, None, &[], 16, 8.0);
        let hr_row = visible_display_row(&fd, 1).expect("HR source line unfolded");
        assert!(
            hr_row > 1,
            "expected wrap to push the HR past source-line index 1, got display row {hr_row}"
        );
        // `---` is 3 chars wide which exceeds the 16 dip wrap width at 8
        // dip/char, so the HR source line itself contributes more than
        // one display row. The painted span runs from the HR's first
        // display row through the row after its last wrap segment.
        let hr_row_count = fd.display_line_count_for_source(1);
        assert!(hr_row_count >= 1);
        let (top, bottom) = block_display_span(&fd, 1, 1, 20.0, 0.0).unwrap();
        let expected_top = hr_row as f32 * 20.0;
        let expected_bottom = (hr_row + hr_row_count) as f32 * 20.0;
        assert!(
            (top - expected_top).abs() < 1e-3,
            "top={top} expected={expected_top}"
        );
        assert!(
            (bottom - expected_bottom).abs() < 1e-3,
            "bottom={bottom} expected={expected_bottom}"
        );
    }
}
