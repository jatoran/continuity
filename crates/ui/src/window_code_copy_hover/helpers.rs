//! Pure-function helpers + tests for the code copy hover affordance.
//! Extracted from the parent module so the `Window` impl + per-frame
//! draw builder stay under the 600-line conventions cap.

use continuity_decorate::BlockSpan;
use continuity_render::FrameDisplay;
use ropey::Rope;

use super::{
    COPY_BUTTON_HEIGHT_DIP, COPY_BUTTON_INSET_RIGHT_DIP, COPY_BUTTON_INSET_TOP_DIP,
    COPY_BUTTON_WIDTH_DIP, INLINE_COPY_BUTTON_HEIGHT_DIP, INLINE_COPY_BUTTON_WIDTH_DIP,
};

/// Inclusive-left / inclusive-top / exclusive-right / exclusive-bottom
/// rect containment for `(x, y, w, h)` rects.
pub(super) fn rect_contains((x, y, w, h): (f32, f32, f32, f32), px: f32, py: f32) -> bool {
    px >= x && px < x + w && py >= y && py < y + h
}

/// Expand a rect by `slop` DIPs on every side. Used to add a small
/// hover slop margin around the inline-code span so the cursor can
/// travel from the span to the button rect without losing focus.
pub(super) fn expand_rect((x, y, w, h): (f32, f32, f32, f32), slop: f32) -> (f32, f32, f32, f32) {
    (
        x - slop,
        y - slop,
        (w + 2.0 * slop).max(0.0),
        (h + 2.0 * slop).max(0.0),
    )
}

/// Top/bottom of `block` in **body-local** DIPs (caller adds the body's
/// y origin to translate into client DIPs). Mirrors
/// `crate::decoration_paint::block_display_span` but is reimplemented
/// here so the UI side does not depend on render-internal helpers.
pub(super) fn block_client_span(
    frame_display: &FrameDisplay,
    rope: &Rope,
    block: &BlockSpan,
    line_height: f32,
    scroll_y_dip: f32,
) -> Option<(f32, f32)> {
    let first_source = byte_to_line(rope, block.start_byte);
    let last_source = byte_to_line(rope, block.end_byte.saturating_sub(1));
    source_lines_client_span(
        frame_display,
        first_source,
        last_source,
        line_height,
        scroll_y_dip,
    )
}

fn source_lines_client_span(
    frame_display: &FrameDisplay,
    first_source: usize,
    last_source: usize,
    line_height: f32,
    scroll_y_dip: f32,
) -> Option<(f32, f32)> {
    let top_row = visible_display_row(frame_display, first_source)?;
    let mut cursor = last_source;
    let bottom_row = loop {
        let count = frame_display.display_line_count_for_source(cursor);
        if count > 0 {
            let first = frame_display.first_display_line_index_for_source(cursor);
            break first + count;
        }
        if cursor == first_source {
            return None;
        }
        cursor -= 1;
    };
    let top = top_row as f32 * line_height - scroll_y_dip;
    let bottom = bottom_row as f32 * line_height - scroll_y_dip;
    Some((top, bottom))
}

/// Top-right inset rect for a fenced block whose painted left/right
/// edges are `(block_left, block_right)` and whose button top is
/// `button_top` — all in client DIPs.
pub(super) fn button_rect_for_block(
    block_left: f32,
    block_right: f32,
    button_top: f32,
) -> (f32, f32, f32, f32) {
    let x = (block_right - COPY_BUTTON_INSET_RIGHT_DIP - COPY_BUTTON_WIDTH_DIP).max(block_left);
    let y = button_top + COPY_BUTTON_INSET_TOP_DIP;
    (x, y, COPY_BUTTON_WIDTH_DIP, COPY_BUTTON_HEIGHT_DIP)
}

/// Inline-button rect anchored inside the inline-code span's top-right
/// corner, in client DIPs. The chip overlaps the rendered code
/// background instead of reserving or borrowing space above the row.
pub(super) fn inline_button_rect(span_rect: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    let (sx, sy, sw, sh) = span_rect;
    let right_x = sx + sw;
    let x = (right_x - INLINE_COPY_BUTTON_WIDTH_DIP).max(sx).max(0.0);
    let y = (sy + ((sh - INLINE_COPY_BUTTON_HEIGHT_DIP) * 0.5).max(0.0)).max(0.0);
    (
        x,
        y,
        INLINE_COPY_BUTTON_WIDTH_DIP,
        INLINE_COPY_BUTTON_HEIGHT_DIP,
    )
}

fn visible_display_row(frame_display: &FrameDisplay, source_line: usize) -> Option<u32> {
    if frame_display.display_line_count_for_source(source_line) == 0 {
        return None;
    }
    Some(frame_display.first_display_line_index_for_source(source_line))
}

fn byte_to_line(rope: &Rope, byte: usize) -> usize {
    let clamped = byte.min(rope.len_bytes());
    rope.byte_to_line(clamped)
}

/// Extract the inner content of a fenced code block (no fence ticks,
/// no info string, no trailing close fence). Used by the click
/// handler when committing to the clipboard; also surfaced via the
/// hover state so the renderer never has to walk the rope itself.
///
/// Strategy: skip the first line (the opening fence with optional
/// info string) and skip the final fence-only line if present.
/// Trailing newline is preserved so multi-line copies paste with
/// the right line breaks.
pub(super) fn fenced_inner_text(rope: &Rope, start_byte: usize, end_byte: usize) -> String {
    let len = rope.len_bytes();
    if start_byte >= end_byte || start_byte >= len {
        return String::new();
    }
    let end = end_byte.min(len);
    let block_text: String = rope.byte_slice(start_byte..end).to_string();
    let bytes = block_text.as_bytes();
    if bytes.is_empty() {
        return String::new();
    }
    let mut first_nl = 0usize;
    while first_nl < bytes.len() && bytes[first_nl] != b'\n' {
        first_nl += 1;
    }
    if first_nl >= bytes.len() {
        return String::new();
    }
    let inner_start = first_nl + 1;
    let mut tail = bytes.len();
    while tail > 0 && matches!(bytes[tail - 1], b'\n' | b'\r') {
        tail -= 1;
    }
    let mut last_line_start = tail;
    while last_line_start > 0 && bytes[last_line_start - 1] != b'\n' {
        last_line_start -= 1;
    }
    let last_line = &bytes[last_line_start..tail];
    let inner_end = if last_line
        .iter()
        .all(|b| matches!(*b, b'`' | b'~' | b' ' | b'\t'))
        && last_line_start >= inner_start
    {
        last_line_start
    } else {
        bytes.len()
    };
    if inner_end <= inner_start {
        return String::new();
    }
    String::from_utf8_lossy(&bytes[inner_start..inner_end]).into_owned()
}

/// Extract the optional info string (language tag) from the fence's
/// opening line — used by the `event:code_copy` trace.
pub(super) fn fence_info_string(rope: &Rope, start_byte: usize, end_byte: usize) -> Option<String> {
    let len = rope.len_bytes();
    if start_byte >= end_byte || start_byte >= len {
        return None;
    }
    let end = end_byte.min(len);
    let block_text: String = rope.byte_slice(start_byte..end).to_string();
    let first_line = block_text.lines().next()?;
    let trimmed = first_line.trim_start();
    let after_fence = trimmed.trim_start_matches(['`', '~']);
    let info = after_fence.trim();
    if info.is_empty() {
        None
    } else {
        Some(info.to_string())
    }
}

/// Inner content of an inline `` `code` `` span — the user-facing
/// text without the backtick delimiters. Reads the rope directly
/// using the inner-byte range that the painter recorded.
pub(super) fn inline_code_inner_text(
    rope: &Rope,
    inner_start_byte: usize,
    inner_end_byte: usize,
) -> String {
    let len = rope.len_bytes();
    if inner_start_byte >= inner_end_byte || inner_start_byte >= len {
        return String::new();
    }
    let end = inner_end_byte.min(len);
    rope.byte_slice(inner_start_byte..end).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_decorate::{BlockKind, BlockSpan};

    fn caret_inside_block(caret_bytes: &[usize], block: &BlockSpan) -> bool {
        caret_bytes
            .iter()
            .any(|b| *b >= block.start_byte && *b <= block.end_byte)
    }

    #[test]
    fn fenced_inner_text_strips_opening_and_closing_fence() {
        let rope = Rope::from_str("```rust\nfn main() {}\n```\n");
        let inner = fenced_inner_text(&rope, 0, rope.len_bytes());
        assert_eq!(inner, "fn main() {}\n");
    }

    #[test]
    fn fenced_inner_text_handles_multi_line_body() {
        let rope = Rope::from_str("```\nline a\nline b\nline c\n```\n");
        let inner = fenced_inner_text(&rope, 0, rope.len_bytes());
        assert_eq!(inner, "line a\nline b\nline c\n");
    }

    #[test]
    fn fenced_inner_text_handles_no_info_string() {
        let rope = Rope::from_str("```\nhi\n```\n");
        let inner = fenced_inner_text(&rope, 0, rope.len_bytes());
        assert_eq!(inner, "hi\n");
    }

    #[test]
    fn fenced_inner_text_empty_block_returns_empty() {
        let rope = Rope::from_str("```\n```\n");
        let inner = fenced_inner_text(&rope, 0, rope.len_bytes());
        assert_eq!(inner, "");
    }

    #[test]
    fn fenced_inner_text_degenerate_single_line_returns_empty() {
        let rope = Rope::from_str("```rust");
        let inner = fenced_inner_text(&rope, 0, rope.len_bytes());
        assert_eq!(inner, "");
    }

    #[test]
    fn fenced_inner_text_keeps_inner_when_no_closing_fence() {
        let rope = Rope::from_str("```\nbody\n");
        let inner = fenced_inner_text(&rope, 0, rope.len_bytes());
        assert_eq!(inner, "body\n");
    }

    #[test]
    fn fence_info_string_extracts_language_tag() {
        let rope = Rope::from_str("```rust\nfn main() {}\n```\n");
        assert_eq!(
            fence_info_string(&rope, 0, rope.len_bytes()).as_deref(),
            Some("rust")
        );
    }

    #[test]
    fn fence_info_string_none_when_absent() {
        let rope = Rope::from_str("```\nhi\n```\n");
        assert_eq!(fence_info_string(&rope, 0, rope.len_bytes()), None);
    }

    #[test]
    fn inline_code_inner_text_slices_exact_bytes() {
        let rope = Rope::from_str("see `let x = 1;` here");
        // Backticks live at byte 4 and 15; inner range is 5..15.
        let inner = inline_code_inner_text(&rope, 5, 15);
        assert_eq!(inner, "let x = 1;");
    }

    #[test]
    fn inline_code_inner_text_empty_range_returns_empty() {
        let rope = Rope::from_str("``");
        assert_eq!(inline_code_inner_text(&rope, 1, 1), "");
    }

    #[test]
    fn button_rect_clamps_to_block_left_on_narrow_blocks() {
        let rect = button_rect_for_block(100.0, 110.0, 200.0);
        assert!(
            (rect.0 - 100.0).abs() < 1e-3,
            "expected clamp to block_left, got {}",
            rect.0
        );
        assert!((rect.1 - (200.0 + COPY_BUTTON_INSET_TOP_DIP)).abs() < 1e-3);
    }

    #[test]
    fn button_rect_sits_at_top_right_on_wide_blocks() {
        let rect = button_rect_for_block(0.0, 400.0, 100.0);
        let expected_x = 400.0 - COPY_BUTTON_INSET_RIGHT_DIP - COPY_BUTTON_WIDTH_DIP;
        assert!((rect.0 - expected_x).abs() < 1e-3, "x={}", rect.0);
        assert!(
            (rect.1 - (100.0 + COPY_BUTTON_INSET_TOP_DIP)).abs() < 1e-3,
            "y={}",
            rect.1
        );
        assert!((rect.2 - COPY_BUTTON_WIDTH_DIP).abs() < 1e-3);
        assert!((rect.3 - COPY_BUTTON_HEIGHT_DIP).abs() < 1e-3);
    }

    #[test]
    fn inline_button_rect_overlaps_span_top_right() {
        // 100 dip wide span at (50, 80).
        let rect = inline_button_rect((50.0, 80.0, 100.0, 18.0));
        let expected_x = 150.0 - INLINE_COPY_BUTTON_WIDTH_DIP;
        assert!((rect.0 - expected_x).abs() < 1e-3, "x={}", rect.0);
        let expected_y = 80.0 + (18.0 - INLINE_COPY_BUTTON_HEIGHT_DIP) * 0.5;
        assert!((rect.1 - expected_y).abs() < 1e-3, "y={}", rect.1);
        assert!((rect.2 - INLINE_COPY_BUTTON_WIDTH_DIP).abs() < 1e-3);
        assert!((rect.3 - INLINE_COPY_BUTTON_HEIGHT_DIP).abs() < 1e-3);
    }

    #[test]
    fn inline_button_rect_clamps_to_span_left_on_narrow_spans() {
        // Span narrower than the button: fall back to anchoring at left.
        let rect = inline_button_rect((10.0, 50.0, 6.0, 18.0));
        assert!(
            (rect.0 - 10.0).abs() < 1e-3,
            "expected clamp to span left, got {}",
            rect.0
        );
    }

    #[test]
    fn expand_rect_grows_by_slop_on_every_side() {
        let r = expand_rect((10.0, 20.0, 30.0, 40.0), 4.0);
        assert!((r.0 - 6.0).abs() < 1e-6);
        assert!((r.1 - 16.0).abs() < 1e-6);
        assert!((r.2 - 38.0).abs() < 1e-6);
        assert!((r.3 - 48.0).abs() < 1e-6);
    }

    #[test]
    fn rect_contains_inclusive_left_top_exclusive_right_bottom() {
        let r = (10.0_f32, 20.0_f32, 30.0_f32, 40.0_f32);
        assert!(rect_contains(r, 10.0, 20.0));
        assert!(rect_contains(r, 39.999, 59.999));
        assert!(!rect_contains(r, 40.0, 50.0));
        assert!(!rect_contains(r, 30.0, 60.0));
        assert!(!rect_contains(r, 9.999, 30.0));
    }

    #[test]
    fn caret_inside_block_inclusive_of_end_byte() {
        let block = BlockSpan {
            kind: BlockKind::FencedCodeBlock,
            start_byte: 10,
            end_byte: 30,
        };
        assert!(caret_inside_block(&[15], &block));
        assert!(caret_inside_block(&[10], &block));
        assert!(caret_inside_block(&[30], &block));
        assert!(!caret_inside_block(&[31], &block));
        assert!(!caret_inside_block(&[9], &block));
    }
}
