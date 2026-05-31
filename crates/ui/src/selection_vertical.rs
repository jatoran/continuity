//! Sticky-column vertical motion helpers (Phase B2).
//!
//! Extracted from [`crate::selection`] to keep that file under the
//! 600-line cap. The state itself (`intended_columns`,
//! `intended_columns_for`) lives on `crate::Window`; this module
//! provides the pure positioning helper plus its tests.

use continuity_display_map::{DisplayByte, SourceByte};
use continuity_render::FrameDisplay;
use continuity_text::Position;
use ropey::Rope;

/// Byte offset of the last source byte before the line's terminator
/// (CR / LF / CRLF). Same semantics as the private helper in
/// `crate::selection` — re-exported here so the column-clamp function
/// is self-contained and unit-testable.
pub(crate) fn line_content_end(rope: &Rope, line: usize) -> usize {
    let start = rope.line_to_byte(line);
    let next = if line + 1 < rope.len_lines() {
        rope.line_to_byte(line + 1)
    } else {
        rope.len_bytes()
    };
    let mut end = next;
    let text = rope.byte_slice(start..next).to_string();
    if text.ends_with('\n') {
        end = end.saturating_sub(1);
        if text.ends_with("\r\n") {
            end = end.saturating_sub(1);
        }
    }
    end
}

/// Vertical motion that clamps to `intended_col` on lines where that
/// column is available. Lines shorter than `intended_col` clip to EOL
/// but the caller's intended-column memory survives so a wider next
/// line restores it. See Phase B2 in the future-updates doc.
/// Visual-row vertical motion. Soft-wrap aware — `delta` is measured
/// in *display rows* rather than source lines, so Shift+Up on a long
/// wrapped paragraph steps to the previous on-screen row instead of
/// jumping past the whole paragraph.
///
/// `intended_display_byte` is the desired display-byte offset within
/// the target row's display string. Callers typically pass the head's
/// current display-byte offset so the caret tracks horizontally when
/// stepping across rows of similar width.
///
/// Falls back to source-line stepping when the projection can't
/// resolve `head` (folded source line, or the rope was empty when the
/// projection was built).
pub(crate) fn move_visual_row(
    rope: &Rope,
    fd: &FrameDisplay,
    head: Position,
    delta: i32,
    intended_display_byte: u32,
) -> Position {
    let source_line = head.line as usize;
    if source_line >= rope.len_lines() {
        return move_line_with_column(rope, head, delta, head.byte_in_line);
    }
    let line_start = rope.line_to_byte(source_line);
    let head_abs = line_start + head.byte_in_line as usize;

    let first = fd.first_display_line_index_for_source(source_line);
    let count = fd.display_line_count_for_source(source_line);
    if count == 0 {
        return move_line_with_column(rope, head, delta, head.byte_in_line);
    }

    // Locate the display row that owns the head byte. A caret exactly
    // at a wrap break point belongs on the *later* row (matches
    // wrap_paint's caret routing).
    let mut current_row = first;
    let end_excl = first + count;
    for i in first..end_excl {
        let Some(spec) = fd.display_line_by_index(i) else {
            continue;
        };
        let s = spec.source_byte_start.raw() as usize;
        let e = spec.source_byte_end.raw() as usize;
        let within = if i + 1 < end_excl {
            head_abs >= s && head_abs < e
        } else {
            head_abs >= s && head_abs <= e
        };
        if within {
            current_row = i;
            break;
        }
    }

    let target_row_raw = current_row as i64 + delta as i64;
    let total = fd.display_line_count() as i64;
    if total <= 0 {
        return move_line_with_column(rope, head, delta, head.byte_in_line);
    }
    if target_row_raw < 0 {
        // Above the document: clamp to byte 0.
        return Position::from_byte_offset(rope, 0).unwrap_or(head);
    }
    if target_row_raw >= total {
        // Below the document: clamp to last source line's content end.
        let last_line = rope.len_lines().saturating_sub(1).max(0);
        let end = line_content_end(rope, last_line);
        return Position::from_byte_offset(rope, end).unwrap_or(head);
    }
    let target = target_row_raw as u32;
    let Some(spec) = fd.display_line_by_index(target) else {
        return head;
    };

    // Clamp intended col to the row's display length, then walk back
    // from there until we find a display byte that maps to a source
    // byte (mid-grapheme / inside a hidden span may return None).
    let max_db = spec.display_len();
    let want = intended_display_byte.min(max_db) as usize;
    let source_byte = (0..=want)
        .rev()
        .find_map(|db| spec.display_to_source(DisplayByte::from_usize(db)))
        .map(|s| s.raw() as usize)
        .unwrap_or_else(|| spec.source_byte_start.raw() as usize);
    Position::from_byte_offset(rope, source_byte).unwrap_or(head)
}

/// Display-byte offset of `head` within its owning display row, or
/// `None` when the projection can't resolve the head (folded line).
/// Returned values can be passed to [`move_visual_row`] as
/// `intended_display_byte`.
pub(crate) fn head_display_byte_in_row(
    rope: &Rope,
    fd: &FrameDisplay,
    head: Position,
) -> Option<u32> {
    let source_line = head.line as usize;
    if source_line >= rope.len_lines() {
        return None;
    }
    let line_start = rope.line_to_byte(source_line);
    let head_abs = line_start + head.byte_in_line as usize;
    let first = fd.first_display_line_index_for_source(source_line);
    let count = fd.display_line_count_for_source(source_line);
    if count == 0 {
        return None;
    }
    let end_excl = first + count;
    for i in first..end_excl {
        let Some(spec) = fd.display_line_by_index(i) else {
            continue;
        };
        let s = spec.source_byte_start.raw() as usize;
        let e = spec.source_byte_end.raw() as usize;
        let within = if i + 1 < end_excl {
            head_abs >= s && head_abs < e
        } else {
            head_abs >= s && head_abs <= e
        };
        if within {
            return spec
                .source_to_display(SourceByte::from_usize(head_abs))
                .map(DisplayByte::raw);
        }
    }
    None
}

pub(crate) fn move_line_with_column(
    rope: &Rope,
    position: Position,
    delta: i32,
    intended_col: u32,
) -> Position {
    let max_line = rope.len_lines().saturating_sub(1).max(0);
    let target_line_raw = position.line as i64 + i64::from(delta);

    if target_line_raw < 0 {
        let start = rope.line_to_byte(0);
        return Position::from_byte_offset(rope, start).unwrap_or(position);
    }
    if (target_line_raw as usize) > max_line {
        let start = rope.line_to_byte(max_line);
        let end = line_content_end(rope, max_line);
        let byte = end.max(start);
        return Position::from_byte_offset(rope, byte).unwrap_or(position);
    }
    let line = target_line_raw as usize;
    let start = rope.line_to_byte(line);
    let end = line_content_end(rope, line);
    let byte = (start + intended_col as usize).min(end);
    Position::from_byte_offset(rope, byte).unwrap_or(position)
}

#[cfg(test)]
mod tests {
    use super::{line_content_end, move_line_with_column};
    use continuity_text::Position;
    use ropey::Rope;

    fn rope(s: &str) -> Rope {
        Rope::from_str(s)
    }

    #[test]
    fn vertical_motion_clips_to_eol_on_short_line() {
        let r = rope("abcdef\nxy\nlongerline");
        let start = Position::new(0, 5);
        let down = move_line_with_column(&r, start, 1, 5);
        assert_eq!(down.line, 1);
        assert_eq!(down.byte_in_line, 2);
    }

    #[test]
    fn vertical_motion_restores_intended_column_on_wider_line() {
        let r = rope("abcdef\nxy\nlongerline");
        let on_line1 = Position::new(1, 2);
        let down = move_line_with_column(&r, on_line1, 1, 5);
        assert_eq!(down.line, 2);
        assert_eq!(down.byte_in_line, 5);
    }

    #[test]
    fn vertical_motion_uses_current_column_when_no_memory() {
        let r = rope("abcdef\nlongerline");
        let start = Position::new(0, 3);
        let down = move_line_with_column(&r, start, 1, 3);
        assert_eq!(down.line, 1);
        assert_eq!(down.byte_in_line, 3);
    }

    #[test]
    fn vertical_motion_past_eof_lands_at_last_line_end() {
        let r = rope("abc\ndef");
        let from = Position::new(1, 1);
        let down = move_line_with_column(&r, from, 1, 1);
        assert_eq!(down.line, 1);
        assert_eq!(down.byte_in_line, 3);
    }

    #[test]
    fn vertical_motion_past_bof_lands_at_doc_start() {
        let r = rope("abcdef\nghi");
        let from = Position::new(0, 2);
        let up = move_line_with_column(&r, from, -1, 2);
        assert_eq!(up.line, 0);
        assert_eq!(up.byte_in_line, 0);
    }

    #[test]
    fn intended_column_zero_is_well_defined() {
        let r = rope("abc\nxyz");
        let from = Position::new(0, 0);
        let down = move_line_with_column(&r, from, 1, 0);
        assert_eq!(down, Position::new(1, 0));
    }

    #[test]
    fn line_content_end_respects_crlf() {
        let r = rope("abc\r\ndef\n");
        assert_eq!(line_content_end(&r, 0), 3);
        assert_eq!(line_content_end(&r, 1), 8);
    }
}
