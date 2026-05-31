//! Selection helpers over a `ropey::Rope`.

use ropey::Rope;

use crate::{Position, Range, Selection, SelectionKind};

/// Select the word around `position`, falling back to a caret when no word
/// byte touches the position.
pub fn word_at(rope: &Rope, position: Position) -> Selection {
    let text = rope.to_string();
    let byte = position.to_byte_offset(rope).unwrap_or(0).min(text.len());
    let Some((start, end)) = word_bounds(&text, byte) else {
        return Selection::caret_at(position);
    };
    selection_from_bytes(rope, start, end, SelectionKind::Caret)
}

/// Select the whole source line containing `position`, excluding the line
/// ending.
pub fn line_at(rope: &Rope, position: Position) -> Selection {
    let line = (position.line as usize).min(rope.len_lines().saturating_sub(1));
    let start = rope.line_to_byte(line);
    let end = line_content_end(rope, line);
    selection_from_bytes(rope, start, end, SelectionKind::LineWise)
}

/// Select the paragraph around `position`. Paragraphs are separated by blank
/// lines.
pub fn paragraph_at(rope: &Rope, position: Position) -> Selection {
    if rope.len_bytes() == 0 {
        return Selection::caret_at(Position::ZERO);
    }
    let mut line = (position.line as usize).min(rope.len_lines().saturating_sub(1));
    if is_blank_line(rope, line) && line > 0 {
        line -= 1;
    }
    let mut start_line = line;
    while start_line > 0 && !is_blank_line(rope, start_line - 1) {
        start_line -= 1;
    }
    let mut end_line = line;
    while end_line + 1 < rope.len_lines() && !is_blank_line(rope, end_line + 1) {
        end_line += 1;
    }
    selection_from_bytes(
        rope,
        rope.line_to_byte(start_line),
        line_content_end(rope, end_line),
        SelectionKind::Caret,
    )
}

/// Expand a selection by prose-oriented scopes: word, sentence, paragraph,
/// then document.
pub fn expand_smart(rope: &Rope, selection: Selection) -> Selection {
    let current = selection.ordered_range();
    let head = selection.head;
    for candidate in [
        word_at(rope, head),
        sentence_at(rope, head),
        paragraph_at(rope, head),
        document(rope),
    ] {
        if contains_range(candidate.ordered_range(), current)
            && candidate.ordered_range() != current
        {
            return candidate;
        }
    }
    document(rope)
}

/// Select the whole document.
pub fn document(rope: &Rope) -> Selection {
    let end = Position::from_byte_offset(rope, rope.len_bytes()).unwrap_or(Position::ZERO);
    Selection::new(Position::ZERO, end, SelectionKind::Caret)
}

fn sentence_at(rope: &Rope, position: Position) -> Selection {
    let text = rope.to_string();
    if text.is_empty() {
        return Selection::caret_at(Position::ZERO);
    }
    let byte = position.to_byte_offset(rope).unwrap_or(0).min(text.len());
    let mut start = 0;
    for (idx, ch) in text[..byte].char_indices().rev() {
        if matches!(ch, '.' | '!' | '?') {
            start = idx + ch.len_utf8();
            break;
        }
    }
    while start < text.len() && text[start..].starts_with(char::is_whitespace) {
        start += text[start..].chars().next().map_or(0, char::len_utf8);
    }
    let mut end = text.len();
    for (offset, ch) in text[byte..].char_indices() {
        if matches!(ch, '.' | '!' | '?') {
            end = byte + offset + ch.len_utf8();
            break;
        }
    }
    selection_from_bytes(rope, start, end, SelectionKind::Caret)
}

fn word_bounds(text: &str, byte: usize) -> Option<(usize, usize)> {
    if text.is_empty() {
        return None;
    }
    let mut probe = byte.min(text.len());
    if probe == text.len() && probe > 0 {
        probe = previous_boundary(text, probe);
    }
    if !is_word_byte(text, probe) && probe > 0 {
        let prev = previous_boundary(text, probe);
        if is_word_byte(text, prev) {
            probe = prev;
        }
    }
    if !is_word_byte(text, probe) {
        return None;
    }
    let mut start = probe;
    while start > 0 {
        let prev = previous_boundary(text, start);
        if !is_word_byte(text, prev) {
            break;
        }
        start = prev;
    }
    let mut end = next_boundary(text, probe);
    while end < text.len() && is_word_byte(text, end) {
        end = next_boundary(text, end);
    }
    Some((start, end))
}

fn is_word_byte(text: &str, byte: usize) -> bool {
    text.get(byte..)
        .and_then(|s| s.chars().next())
        .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
}

fn previous_boundary(text: &str, byte: usize) -> usize {
    text[..byte].char_indices().last().map_or(0, |(idx, _)| idx)
}

fn next_boundary(text: &str, byte: usize) -> usize {
    text[byte..]
        .chars()
        .next()
        .map_or(text.len(), |ch| byte + ch.len_utf8())
}

fn selection_from_bytes(rope: &Rope, start: usize, end: usize, kind: SelectionKind) -> Selection {
    let anchor = Position::from_byte_offset(rope, start).unwrap_or(Position::ZERO);
    let head = Position::from_byte_offset(rope, end).unwrap_or(anchor);
    Selection::new(anchor, head, kind)
}

fn line_content_end(rope: &Rope, line: usize) -> usize {
    let start = rope.line_to_byte(line);
    let next = if line + 1 < rope.len_lines() {
        rope.line_to_byte(line + 1)
    } else {
        rope.len_bytes()
    };
    let mut end = next;
    let slice = rope.byte_slice(start..next).to_string();
    if slice.ends_with('\n') {
        end = end.saturating_sub(1);
        if slice.ends_with("\r\n") {
            end = end.saturating_sub(1);
        }
    }
    end
}

fn is_blank_line(rope: &Rope, line: usize) -> bool {
    let start = rope.line_to_byte(line);
    let end = line_content_end(rope, line);
    rope.byte_slice(start..end).to_string().trim().is_empty()
}

fn contains_range(outer: Range, inner: Range) -> bool {
    outer.start <= inner.start && outer.end >= inner.end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_selects_identifier() {
        let rope = Rope::from_str("hello world");
        let sel = word_at(&rope, Position::new(0, 7));
        assert_eq!(sel.anchor, Position::new(0, 6));
        assert_eq!(sel.head, Position::new(0, 11));
    }

    #[test]
    fn paragraph_spans_until_blank_lines() {
        let rope = Rope::from_str("a\nb\n\nc\n");
        let sel = paragraph_at(&rope, Position::new(1, 0));
        assert_eq!(sel.anchor, Position::ZERO);
        assert_eq!(sel.head, Position::new(1, 1));
    }
}
