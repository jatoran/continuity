//! Find/replace edit planning helpers.

use continuity_text::{EditOp, Position, Range, Selection};
use ropey::Rope;

const FULL_BUFFER_REPLACE_THRESHOLD: usize = 512;

/// Interpret the C-style escapes `\n`, `\t`, `\r`, and `\\` in a replacement
/// string. Used only when the find bar is in regex mode, mirroring
/// Sublime/ripgrep: in regex mode a typed two-char `\n` inserts a newline,
/// `\t` a tab, `\r` a carriage return, and `\\` a single backslash. Any other
/// `\x` sequence is preserved verbatim (backslash kept) so unknown escapes are
/// not silently dropped.
///
/// In literal (non-regex) mode the replacement is taken as-is — callers must
/// not invoke this — so a literal `\n` stays two characters.
#[must_use]
pub(crate) fn interpret_replacement_escapes(replacement: &str) -> String {
    if !replacement.contains('\\') {
        return replacement.to_string();
    }
    let mut out = String::with_capacity(replacement.len());
    let mut chars = replacement.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                // Unknown escape: keep the backslash and the following
                // character verbatim rather than dropping either.
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Build descending replace ops and a conservative post-edit caret.
///
/// `interpret_escapes` is set when the find bar is in regex mode: the
/// `replacement` then has its `\n` / `\t` / `\r` / `\\` escapes expanded
/// before insertion (so a regex replace can emit newlines). In literal mode
/// it is `false` and the replacement is used verbatim.
pub(crate) fn build_replace_all_plan(
    rope: &Rope,
    ranges: &[(usize, usize)],
    replacement: &str,
    preserve_case: bool,
    interpret_escapes: bool,
) -> (Vec<EditOp>, Vec<Selection>) {
    let expanded;
    let replacement = if interpret_escapes {
        expanded = interpret_replacement_escapes(replacement);
        expanded.as_str()
    } else {
        replacement
    };
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
///
/// `interpret_escapes` mirrors [`build_replace_all_plan`]: set in regex mode
/// so the replacement's `\n` / `\t` / `\r` / `\\` escapes expand before
/// insertion; `false` in literal mode for verbatim text.
pub(crate) fn replacement_for_one(
    rope: &Rope,
    range: (usize, usize),
    replacement: &str,
    preserve_case: bool,
    interpret_escapes: bool,
) -> String {
    let expanded;
    let replacement = if interpret_escapes {
        expanded = interpret_replacement_escapes(replacement);
        expanded.as_str()
    } else {
        replacement
    };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_expands_newline_tab_return_and_backslash() {
        assert_eq!(interpret_replacement_escapes(r"a\nb"), "a\nb");
        assert_eq!(interpret_replacement_escapes(r"a\tb"), "a\tb");
        assert_eq!(interpret_replacement_escapes(r"a\rb"), "a\rb");
        assert_eq!(interpret_replacement_escapes(r"a\\b"), r"a\b");
    }

    #[test]
    fn escape_unknown_sequence_is_preserved_verbatim() {
        // `\q` is not a recognized escape: keep both characters.
        assert_eq!(interpret_replacement_escapes(r"a\qb"), r"a\qb");
    }

    #[test]
    fn escape_trailing_backslash_is_preserved() {
        assert_eq!(interpret_replacement_escapes(r"end\"), r"end\");
    }

    #[test]
    fn escape_noop_when_no_backslash() {
        assert_eq!(interpret_replacement_escapes("plain text"), "plain text");
    }

    #[test]
    fn replacement_for_one_interprets_escape_only_in_regex_mode() {
        let rope = Rope::from_str("hello world");
        let range = (0usize, 5usize); // "hello"
                                      // Regex mode: `\n` expands to a newline.
        let regex = replacement_for_one(&rope, range, r"a\nb", false, true);
        assert_eq!(regex, "a\nb");
        // Literal mode: the two-char escape stays verbatim.
        let literal = replacement_for_one(&rope, range, r"a\nb", false, false);
        assert_eq!(literal, r"a\nb");
    }

    #[test]
    fn replace_all_plan_inserts_newline_in_regex_mode() {
        let rope = Rope::from_str("AB");
        let ranges = [(0usize, 1usize)]; // replace "A"
        let (ops, _) = build_replace_all_plan(&rope, &ranges, r"x\ny", false, true);
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            EditOp::Replace { text, .. } => assert_eq!(text, "x\ny"),
            other => panic!("expected a replace op, got {other:?}"),
        }
    }

    #[test]
    fn replace_all_plan_literal_mode_keeps_escape_verbatim() {
        let rope = Rope::from_str("AB");
        let ranges = [(0usize, 1usize)];
        let (ops, _) = build_replace_all_plan(&rope, &ranges, r"x\ny", false, false);
        match &ops[0] {
            EditOp::Replace { text, .. } => assert_eq!(text, r"x\ny"),
            other => panic!("expected a replace op, got {other:?}"),
        }
    }

    #[test]
    fn replace_all_plan_handles_multiline_match_span() {
        // A match that crosses a newline (the multi-line regex find can
        // produce these) replaces correctly via byte-range positions.
        let rope = Rope::from_str("a\nb tail");
        let ranges = [(0usize, 3usize)]; // "a\nb" spans the newline
        let (ops, selections_after) = build_replace_all_plan(&rope, &ranges, "X", false, false);
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            EditOp::Replace { range, text } => {
                assert_eq!(text, "X");
                assert_eq!(range.start, Position::ZERO);
                // End is on line 1 (0-indexed), byte 1 — the 'b'.
                assert_eq!(range.end.line, 1);
            }
            other => panic!("expected a replace op, got {other:?}"),
        }
        assert_eq!(selections_after.len(), 1);
    }
}
