//! Find/replace edit planning helpers.

use continuity_text::{EditOp, Position, Range, Selection};
use ropey::Rope;

const FULL_BUFFER_REPLACE_THRESHOLD: usize = 512;

/// Build descending replace ops and a conservative post-edit caret.
pub(crate) fn build_replace_all_plan(
    rope: &Rope,
    ranges: &[(usize, usize)],
    replacement: &str,
    preserve_case: bool,
) -> (Vec<EditOp>, Vec<Selection>) {
    let selections_after = ranges
        .first()
        .and_then(|(start, _)| Position::from_byte_offset(rope, *start).ok())
        .map(|pos| vec![Selection::caret_at(pos)])
        .unwrap_or_default();
    if ranges.len() >= FULL_BUFFER_REPLACE_THRESHOLD {
        return (
            build_full_buffer_replace(rope, ranges, replacement, preserve_case),
            selections_after,
        );
    }
    let mut ops = Vec::with_capacity(ranges.len());
    for &(start_byte, end_byte) in ranges.iter().rev() {
        let Ok(start) = Position::from_byte_offset(rope, start_byte) else {
            continue;
        };
        let Ok(end) = Position::from_byte_offset(rope, end_byte) else {
            continue;
        };
        let text = replacement_for_range(rope, start_byte, end_byte, replacement, preserve_case);
        ops.push(EditOp::replace(Range::new(start, end), text));
    }
    (ops, selections_after)
}

/// Build replacement text for one current match.
pub(crate) fn replacement_for_one(
    rope: &Rope,
    range: (usize, usize),
    replacement: &str,
    preserve_case: bool,
) -> String {
    replacement_for_range(rope, range.0, range.1, replacement, preserve_case)
}

fn build_full_buffer_replace(
    rope: &Rope,
    ranges: &[(usize, usize)],
    replacement: &str,
    preserve_case: bool,
) -> Vec<EditOp> {
    let original = rope.to_string();
    let mut out = String::with_capacity(original.len());
    let mut last = 0usize;
    for &(start, end) in ranges {
        if start < last || end > original.len() || start > end {
            return Vec::new();
        }
        out.push_str(&original[last..start]);
        let matched = &original[start..end];
        out.push_str(&apply_preserve_case(matched, replacement, preserve_case));
        last = end;
    }
    out.push_str(&original[last..]);
    if out == original {
        return Vec::new();
    }
    let Ok(end) = Position::from_byte_offset(rope, original.len()) else {
        return Vec::new();
    };
    vec![EditOp::replace(Range::new(Position::ZERO, end), out)]
}

fn replacement_for_range(
    rope: &Rope,
    start: usize,
    end: usize,
    replacement: &str,
    preserve_case: bool,
) -> String {
    if !preserve_case {
        return replacement.to_string();
    }
    let matched = rope.byte_slice(start..end).to_string();
    apply_preserve_case(&matched, replacement, true)
}

fn apply_preserve_case(matched: &str, replacement: &str, preserve_case: bool) -> String {
    if !preserve_case || replacement.is_empty() || !matched.chars().any(char::is_alphabetic) {
        return replacement.to_string();
    }
    if is_all_alphabetic_case(matched, char::is_uppercase) {
        return replacement.to_uppercase();
    }
    if is_all_alphabetic_case(matched, char::is_lowercase) {
        return replacement.to_lowercase();
    }
    if is_title_case(matched) {
        return title_case(replacement);
    }
    replacement.to_string()
}

fn is_all_alphabetic_case(matched: &str, predicate: fn(char) -> bool) -> bool {
    matched.chars().filter(|c| c.is_alphabetic()).all(predicate)
}

fn is_title_case(matched: &str) -> bool {
    let mut letters = matched.chars().filter(|c| c.is_alphabetic());
    let Some(first) = letters.next() else {
        return false;
    };
    first.is_uppercase() && letters.all(char::is_lowercase)
}

fn title_case(replacement: &str) -> String {
    let mut chars = replacement.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut out = first.to_uppercase().collect::<String>();
    out.push_str(&chars.as_str().to_lowercase());
    out
}
