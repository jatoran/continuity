//! Character-scope edits — transpose, change case, indent helpers,
//! surround, wrap-at-column, reflow, delete-to-bracket.
//!
//! These commands operate on the bytes inside (or adjacent to) each
//! selection and never collapse multi-cursor input into a single span.

use continuity_buffer::Buffer;
use continuity_text::{Position, Selection};

use crate::edit_planning::{
    advance_position, finalize_specs, line_content_end, ranges_for_selection, EditSpec,
};
use crate::selection_edit::SelectionEditPlan;
use crate::CaseKind;
use crate::Error;

pub(crate) fn plan_transpose_chars(buffer: &Buffer) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let mut specs = Vec::new();
    let mut selections_after = Vec::new();
    for selection in &selections_before {
        let head = selection.head;
        let byte = head.to_byte_offset(rope)?;
        if byte == 0 || byte >= rope.len_bytes() {
            selections_after.push(*selection);
            continue;
        }
        let char_idx = rope.byte_to_char(byte);
        if char_idx == 0 || char_idx >= rope.len_chars() {
            selections_after.push(*selection);
            continue;
        }
        let prev_byte = rope.char_to_byte(char_idx - 1);
        let next_byte = rope.char_to_byte(char_idx + 1);
        let prev = rope.byte_slice(prev_byte..byte).to_string();
        let next = rope.byte_slice(byte..next_byte).to_string();
        let replaced = format!("{next}{prev}");
        specs.push(EditSpec::replace(rope, prev_byte, next_byte, replaced)?);
        selections_after.push(*selection);
    }
    Ok(finalize_specs(specs, selections_before, selections_after))
}

pub(crate) fn plan_change_case(
    buffer: &Buffer,
    kind: CaseKind,
) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let mut specs = Vec::new();
    let mut selections_after = Vec::new();
    for selection in &selections_before {
        let range = selection.ordered_range();
        let mut start = range.start.to_byte_offset(rope)?;
        let mut end = range.end.to_byte_offset(rope)?;
        if start == end {
            // No selection — operate on the surrounding word.
            let bytes: Vec<u8> = rope.bytes().collect();
            let len = bytes.len();
            let mut s = start.min(len.saturating_sub(1));
            while s > 0 && is_word_byte(bytes[s - 1]) {
                s -= 1;
            }
            let mut e = start;
            while e < len && is_word_byte(bytes[e]) {
                e += 1;
            }
            if s == e {
                selections_after.push(*selection);
                continue;
            }
            start = s;
            end = e;
        }
        let original = rope.byte_slice(start..end).to_string();
        let converted = convert_case(&original, kind);
        if converted == original {
            selections_after.push(*selection);
            continue;
        }
        specs.push(EditSpec::replace(rope, start, end, converted)?);
        selections_after.push(*selection);
    }
    Ok(finalize_specs(specs, selections_before, selections_after))
}

pub(crate) fn plan_surround_selection(
    buffer: &Buffer,
    open: &str,
    close: &str,
) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let mut specs = Vec::new();
    let mut selections_after = Vec::new();
    for selection in &selections_before {
        let range = selection.ordered_range();
        let start = range.start.to_byte_offset(rope)?;
        let end = range.end.to_byte_offset(rope)?;
        if start == end {
            selections_after.push(*selection);
            continue;
        }
        // Two specs: insert open at start, insert close at end. We emit
        // them as one Replace covering the whole span so the planner sorts
        // them as a single unit and the byte ordering is correct.
        let original = rope.byte_slice(start..end).to_string();
        let replaced = format!("{open}{original}{close}");
        specs.push(EditSpec::replace(rope, start, end, replaced.clone())?);
        let new_head = advance_position(range.start, &replaced);
        selections_after.push(Selection::new(range.start, new_head, selection.kind));
    }
    Ok(finalize_specs(specs, selections_before, selections_after))
}

pub(crate) fn plan_wrap_at_column(
    buffer: &Buffer,
    width: u32,
) -> Result<Option<SelectionEditPlan>, Error> {
    rewrite_paragraphs(buffer, width, false)
}

pub(crate) fn plan_reflow_paragraph(
    buffer: &Buffer,
    width: u32,
) -> Result<Option<SelectionEditPlan>, Error> {
    rewrite_paragraphs(buffer, width, true)
}

pub(crate) fn plan_delete_to_bracket(buffer: &Buffer) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let mut specs = Vec::new();
    let mut selections_after = Vec::new();
    let bytes: Vec<u8> = rope.bytes().collect();
    for selection in &selections_before {
        let head = selection.head;
        let byte = head.to_byte_offset(rope)?;
        if let Some(end) = matching_bracket(&bytes, byte) {
            let (start, end) = if end > byte {
                (byte, end + 1)
            } else {
                (end, byte)
            };
            specs.push(EditSpec::delete(rope, start, end)?);
            let position = Position::from_byte_offset(rope, start)?;
            selections_after.push(Selection::caret_at(position));
        } else {
            selections_after.push(*selection);
        }
    }
    Ok(finalize_specs(specs, selections_before, selections_after))
}

fn rewrite_paragraphs(
    buffer: &Buffer,
    width: u32,
    preserve_indent: bool,
) -> Result<Option<SelectionEditPlan>, Error> {
    if width == 0 {
        return Ok(None);
    }
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let mut specs = Vec::new();
    let width = width as usize;
    let mut emitted: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for selection in &selections_before {
        for range in ranges_for_selection(rope, *selection)? {
            let start_line = range.start.line as usize;
            if !emitted.insert(start_line) {
                continue;
            }
            let line_start = rope.line_to_byte(start_line);
            let line_end = line_content_end(rope, start_line);
            let original = rope.byte_slice(line_start..line_end).to_string();
            let indent: String = if preserve_indent {
                original
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .collect()
            } else {
                String::new()
            };
            let body = original[indent.len()..].to_string();
            let wrapped = wrap_text(&body, width.saturating_sub(indent.len()).max(1));
            let mut rebuilt = String::new();
            for (i, line) in wrapped.iter().enumerate() {
                if i > 0 {
                    rebuilt.push('\n');
                }
                if !line.is_empty() {
                    rebuilt.push_str(&indent);
                    rebuilt.push_str(line);
                }
            }
            if rebuilt != original {
                specs.push(EditSpec::replace(rope, line_start, line_end, rebuilt)?);
            }
        }
    }
    Ok(finalize_specs(
        specs,
        selections_before.clone(),
        selections_before,
    ))
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in words {
        if current.is_empty() {
            current.push_str(word);
            continue;
        }
        if current.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
        } else {
            current.push(' ');
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn matching_bracket(bytes: &[u8], at: usize) -> Option<usize> {
    let len = bytes.len();
    if at >= len {
        return None;
    }
    let (open, close, dir) = match bytes[at] {
        b'(' => (b'(', b')', 1_i32),
        b'[' => (b'[', b']', 1_i32),
        b'{' => (b'{', b'}', 1_i32),
        b')' => (b'(', b')', -1_i32),
        b']' => (b'[', b']', -1_i32),
        b'}' => (b'{', b'}', -1_i32),
        _ => return None,
    };
    let mut depth = 1_i32;
    let mut i = at as i64;
    loop {
        i += i64::from(dir);
        if i < 0 || i as usize >= len {
            return None;
        }
        let b = bytes[i as usize];
        if b == open {
            depth += if dir > 0 { 1 } else { -1 };
        } else if b == close {
            depth += if dir > 0 { -1 } else { 1 };
        }
        if depth == 0 {
            return Some(i as usize);
        }
    }
}

fn convert_case(text: &str, kind: CaseKind) -> String {
    match kind {
        CaseKind::Upper => text.to_uppercase(),
        CaseKind::Lower => text.to_lowercase(),
        CaseKind::Toggle => text
            .chars()
            .map(|c| {
                if c.is_uppercase() {
                    c.to_ascii_lowercase()
                } else if c.is_lowercase() {
                    c.to_ascii_uppercase()
                } else {
                    c
                }
            })
            .collect(),
        CaseKind::Title => title_case(text),
        CaseKind::Sentence => sentence_case(text),
    }
}

fn title_case(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_alpha = false;
    for c in text.chars() {
        if c.is_alphabetic() {
            if prev_alpha {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
            prev_alpha = true;
        } else {
            out.push(c);
            prev_alpha = false;
        }
    }
    out
}

fn sentence_case(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut start = true;
    for c in text.chars() {
        if c.is_alphabetic() {
            if start {
                out.extend(c.to_uppercase());
                start = false;
            } else {
                out.extend(c.to_lowercase());
            }
        } else {
            out.push(c);
            if matches!(c, '.' | '!' | '?') {
                start = true;
            }
        }
    }
    out
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

#[cfg(test)]
mod tests {
    use continuity_buffer::Buffer;
    use continuity_text::{Position, Selection, SelectionKind};

    use super::*;
    use crate::selection_edit::{apply_plan, plan, SelectionEdit};

    fn caret(line: u32, col: u32) -> Selection {
        Selection::caret_at(Position::new(line, col))
    }

    fn run(buffer: &mut Buffer, edit: SelectionEdit) {
        let plan = plan(buffer, &edit).expect("plan ok").expect("plan some");
        apply_plan(buffer, &plan).expect("apply ok");
    }

    #[test]
    fn transpose_chars_swaps_pair() {
        let mut b = Buffer::from_text("ab");
        b.set_selections(vec![caret(0, 1)]);
        run(&mut b, SelectionEdit::TransposeChars);
        assert_eq!(b.rope().to_string(), "ba");
    }

    #[test]
    fn change_case_upper_on_word() {
        let mut b = Buffer::from_text("hello");
        b.set_selections(vec![caret(0, 3)]);
        run(&mut b, SelectionEdit::ChangeCase(CaseKind::Upper));
        assert_eq!(b.rope().to_string(), "HELLO");
    }

    #[test]
    fn change_case_title_on_selection() {
        let mut b = Buffer::from_text("hello world");
        b.set_selections(vec![Selection::new(
            Position::new(0, 0),
            Position::new(0, 11),
            SelectionKind::Caret,
        )]);
        run(&mut b, SelectionEdit::ChangeCase(CaseKind::Title));
        assert_eq!(b.rope().to_string(), "Hello World");
    }

    #[test]
    fn change_case_toggle_inverts() {
        let mut b = Buffer::from_text("Hi");
        b.set_selections(vec![Selection::new(
            Position::new(0, 0),
            Position::new(0, 2),
            SelectionKind::Caret,
        )]);
        run(&mut b, SelectionEdit::ChangeCase(CaseKind::Toggle));
        assert_eq!(b.rope().to_string(), "hI");
    }

    #[test]
    fn surround_selection_wraps() {
        let mut b = Buffer::from_text("text");
        b.set_selections(vec![Selection::new(
            Position::new(0, 0),
            Position::new(0, 4),
            SelectionKind::Caret,
        )]);
        run(
            &mut b,
            SelectionEdit::SurroundSelection {
                open: "(".into(),
                close: ")".into(),
            },
        );
        assert_eq!(b.rope().to_string(), "(text)");
    }

    #[test]
    fn wrap_at_column_breaks_long_line() {
        let mut b = Buffer::from_text("one two three four five");
        b.set_selections(vec![caret(0, 0)]);
        run(&mut b, SelectionEdit::WrapAtColumn(10));
        assert_eq!(b.rope().to_string(), "one two\nthree four\nfive");
    }

    #[test]
    fn reflow_paragraph_preserves_indent() {
        let mut b = Buffer::from_text("    one two three four five");
        b.set_selections(vec![caret(0, 0)]);
        run(&mut b, SelectionEdit::ReflowParagraph(14));
        assert_eq!(
            b.rope().to_string(),
            "    one two\n    three four\n    five"
        );
    }

    #[test]
    fn delete_to_bracket_drops_inclusive() {
        let mut b = Buffer::from_text("a(b)c");
        b.set_selections(vec![caret(0, 1)]);
        run(&mut b, SelectionEdit::DeleteToBracket);
        assert_eq!(b.rope().to_string(), "ac");
    }
}
