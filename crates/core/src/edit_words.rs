//! Word-boundary edits — `delete_word_*`, `transpose_words`.
//!
//! Word boundaries match the Phase 5 rule: alphanumeric or `_` runs are
//! words; everything else is a separator. UTF-8 multi-byte characters are
//! treated as part of a word when they are alphanumeric per Unicode.

use continuity_buffer::Buffer;
use continuity_text::Selection;

use crate::edit_planning::{advance_position, EditSpec};
use crate::selection_edit::SelectionEditPlan;
use crate::Error;

/// Delete from each caret backward to the previous word boundary.
pub(crate) fn plan_delete_word_backward(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    delete_word(buffer, true)
}

/// Delete from each caret forward to the next word boundary.
pub(crate) fn plan_delete_word_forward(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    delete_word(buffer, false)
}

/// Swap the words on either side of each caret.
pub(crate) fn plan_transpose_words(buffer: &Buffer) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let mut specs = Vec::new();
    let mut selections_after = Vec::new();
    let len = rope.len_bytes();
    let bytes: Vec<u8> = rope.bytes().collect();

    for selection in &selections_before {
        let head = selection.head;
        let byte = head.to_byte_offset(rope)?;
        let Some((a_start, a_end)) = word_around(&bytes, byte) else {
            selections_after.push(*selection);
            continue;
        };
        let Some((b_start, b_end)) = next_word(&bytes, a_end, len) else {
            selections_after.push(*selection);
            continue;
        };
        let a_text = String::from_utf8_lossy(&bytes[a_start..a_end]).to_string();
        let b_text = String::from_utf8_lossy(&bytes[b_start..b_end]).to_string();
        let between = String::from_utf8_lossy(&bytes[a_end..b_start]).to_string();
        let combined = format!("{b_text}{between}{a_text}");
        specs.push(EditSpec::replace(rope, a_start, b_end, combined.clone())?);
        let start_position = continuity_text::Position::from_byte_offset(rope, a_start)?;
        let new_head = advance_position(start_position, &combined);
        selections_after.push(Selection::caret_at(new_head));
    }

    Ok(crate::edit_planning::finalize_specs(
        specs,
        selections_before,
        selections_after,
    ))
}

fn delete_word(buffer: &Buffer, backward: bool) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let mut specs = Vec::new();
    let mut selections_after = Vec::new();
    let bytes: Vec<u8> = rope.bytes().collect();

    for selection in &selections_before {
        let range = selection.ordered_range();
        let start = range.start.to_byte_offset(rope)?;
        let end = range.end.to_byte_offset(rope)?;
        if start != end {
            specs.push(EditSpec::delete(rope, start, end)?);
            selections_after.push(Selection::caret_at(range.start));
            continue;
        }
        let caret = start;
        let (delete_start, delete_end) = if backward {
            (word_boundary_before(&bytes, caret), caret)
        } else {
            (caret, word_boundary_after(&bytes, caret))
        };
        if delete_start == delete_end {
            selections_after.push(*selection);
            continue;
        }
        specs.push(EditSpec::delete(rope, delete_start, delete_end)?);
        let head = continuity_text::Position::from_byte_offset(rope, delete_start)?;
        selections_after.push(Selection::caret_at(head));
    }

    Ok(crate::edit_planning::finalize_specs(
        specs,
        selections_before,
        selections_after,
    ))
}

/// Find the byte offset of the start of the previous word/separator chunk
/// from `caret`. The behavior matches typical editors: skip whitespace
/// backward, then skip the contiguous run of word-or-non-word bytes.
///
/// A newline is a hard boundary *once whitespace on the current line has
/// been skipped*: deleting the trailing spaces of a blank (or
/// whitespace-only) line stops at the line start rather than swallowing
/// the line break and merging into the line above. When the caret already
/// sits at the line start (no whitespace skipped), the newline is deleted
/// as usual so a bare `Ctrl+Backspace` still joins lines.
fn word_boundary_before(bytes: &[u8], caret: usize) -> usize {
    let mut i = caret;
    let mut skipped_blank = false;
    while i > 0 && is_blank(bytes[i - 1]) {
        i -= 1;
        skipped_blank = true;
    }
    if i == 0 {
        return 0;
    }
    if skipped_blank && is_newline(bytes[i - 1]) {
        return i;
    }
    let word = is_word_byte(bytes[i - 1]);
    while i > 0 && is_word_byte(bytes[i - 1]) == word && !is_blank(bytes[i - 1]) {
        i -= 1;
    }
    i
}

/// Mirror of [`word_boundary_before`] for forward (`Ctrl+Delete`)
/// deletion: a newline is a hard boundary once leading whitespace on the
/// current line has been skipped, so deleting forward over a
/// whitespace-only line stops at the line end instead of pulling the next
/// line up.
fn word_boundary_after(bytes: &[u8], caret: usize) -> usize {
    let len = bytes.len();
    let mut i = caret;
    let mut skipped_blank = false;
    while i < len && is_blank(bytes[i]) {
        i += 1;
        skipped_blank = true;
    }
    if i >= len {
        return len;
    }
    if skipped_blank && is_newline(bytes[i]) {
        return i;
    }
    let word = is_word_byte(bytes[i]);
    while i < len && is_word_byte(bytes[i]) == word && !is_blank(bytes[i]) {
        i += 1;
    }
    i
}

fn word_around(bytes: &[u8], byte: usize) -> Option<(usize, usize)> {
    let len = bytes.len();
    if len == 0 {
        return None;
    }
    let probe = byte.min(len.saturating_sub(1));
    let mut at = probe;
    if !is_word_byte(bytes[at]) {
        // Step back one byte to see if the caret sits just past a word.
        if byte == 0 || !is_word_byte(bytes[byte - 1]) {
            return None;
        }
        at = byte - 1;
    }
    let mut start = at;
    while start > 0 && is_word_byte(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = at;
    while end < len && is_word_byte(bytes[end]) {
        end += 1;
    }
    Some((start, end))
}

fn next_word(bytes: &[u8], from: usize, len: usize) -> Option<(usize, usize)> {
    let mut i = from;
    while i < len && !is_word_byte(bytes[i]) {
        i += 1;
    }
    if i >= len {
        return None;
    }
    let start = i;
    while i < len && is_word_byte(bytes[i]) {
        i += 1;
    }
    Some((start, i))
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

fn is_blank(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

/// A line break that a word-wise delete must not cross once it has begun
/// consuming whitespace on the current line. Covers LF and a lone CR so a
/// whitespace-only line shrinks to its own start instead of merging into
/// its neighbour.
fn is_newline(b: u8) -> bool {
    b == b'\n' || b == b'\r'
}

#[cfg(test)]
mod tests {
    use continuity_buffer::Buffer;
    use continuity_text::{Position, Selection};

    use crate::selection_edit::{apply_plan, plan, SelectionEdit};

    #[test]
    fn delete_word_backward_removes_word_run() {
        let mut buffer = Buffer::from_text("hello world");
        buffer.set_selections(vec![Selection::caret_at(Position::new(0, 11))]);
        let plan = plan(&buffer, &SelectionEdit::DeleteWordBackward)
            .expect("plan ok")
            .expect("plan some");
        apply_plan(&mut buffer, &plan).expect("apply ok");
        assert_eq!(buffer.rope().to_string(), "hello ");
    }

    #[test]
    fn delete_word_forward_skips_spaces_then_word() {
        let mut buffer = Buffer::from_text("alpha   beta");
        buffer.set_selections(vec![Selection::caret_at(Position::new(0, 5))]);
        let plan = plan(&buffer, &SelectionEdit::DeleteWordForward)
            .expect("plan ok")
            .expect("plan some");
        apply_plan(&mut buffer, &plan).expect("apply ok");
        assert_eq!(buffer.rope().to_string(), "alpha");
    }

    #[test]
    fn delete_word_backward_on_blank_line_stops_at_line_start() {
        // Caret at the end of a whitespace-only line: the spaces are
        // removed but the line break above survives — the caret lands at
        // column 0 of the same (now empty) line, never merging upward.
        let mut buffer = Buffer::from_text("abc\n   ");
        buffer.set_selections(vec![Selection::caret_at(Position::new(1, 3))]);
        let plan = plan(&buffer, &SelectionEdit::DeleteWordBackward)
            .expect("plan ok")
            .expect("plan some");
        apply_plan(&mut buffer, &plan).expect("apply ok");
        assert_eq!(buffer.rope().to_string(), "abc\n");
        assert_eq!(buffer.selections()[0].head, Position::new(1, 0));
    }

    #[test]
    fn delete_word_backward_at_line_start_still_merges_up() {
        // Regression guard: with the caret already at column 0 (no
        // whitespace to skip) a backward word delete must still consume
        // the line break and join the previous line.
        let mut buffer = Buffer::from_text("abc\ndef");
        buffer.set_selections(vec![Selection::caret_at(Position::new(1, 0))]);
        let plan = plan(&buffer, &SelectionEdit::DeleteWordBackward)
            .expect("plan ok")
            .expect("plan some");
        apply_plan(&mut buffer, &plan).expect("apply ok");
        assert_eq!(buffer.rope().to_string(), "abcdef");
    }

    #[test]
    fn delete_word_forward_on_blank_line_stops_at_line_end() {
        // Symmetric forward case: Ctrl+Delete over a whitespace-only line
        // removes the spaces but leaves the line break, so the next line
        // is not pulled up.
        let mut buffer = Buffer::from_text("   \nabc");
        buffer.set_selections(vec![Selection::caret_at(Position::new(0, 0))]);
        let plan = plan(&buffer, &SelectionEdit::DeleteWordForward)
            .expect("plan ok")
            .expect("plan some");
        apply_plan(&mut buffer, &plan).expect("apply ok");
        assert_eq!(buffer.rope().to_string(), "\nabc");
    }

    #[test]
    fn transpose_words_swaps_pair() {
        let mut buffer = Buffer::from_text("alpha beta");
        buffer.set_selections(vec![Selection::caret_at(Position::new(0, 3))]);
        let plan = plan(&buffer, &SelectionEdit::TransposeWords)
            .expect("plan ok")
            .expect("plan some");
        apply_plan(&mut buffer, &plan).expect("apply ok");
        assert_eq!(buffer.rope().to_string(), "beta alpha");
    }

    #[test]
    fn transpose_words_at_end_is_noop() {
        let mut buffer = Buffer::from_text("only");
        buffer.set_selections(vec![Selection::caret_at(Position::new(0, 4))]);
        let plan = plan(&buffer, &SelectionEdit::TransposeWords).expect("plan ok");
        assert!(plan.is_none());
    }
}
