//! γ — backslash-escape display rule.
//!
//! Markdown source like `\*literal\*` should render as `*literal*` once
//! the caret leaves the line — the backslashes are syntax, not
//! content. When the caret returns to the line, the backslashes
//! re-appear so the user can see exactly what bytes live in the rope.
//!
//! This module is a sibling display-map provider in the spirit of
//! [`crate::table_hide_provider`]: it produces the per-line byte
//! ranges that the builder should mark `Hidden`. It owes no behaviour
//! to decorations because the backslash escape is a literal source
//! sequence, not a parsed inline span — scanning the line text
//! directly costs one pass and avoids a new decoration kind.
//!
//! The set of recognised escape characters matches the CommonMark
//! list of ASCII-punctuation characters that gain meaning in markdown
//! ( `\` `` ` `` `*` `_` `{` `}` `[` `]` `(` `)` `#` `+` `-` `.` `!`
//! `|` `<` `>` `~` `"` `'` `:` `=` `;` `?` `@` `^` `$` `%` `&` `,`
//! `/` ). Escaped whitespace is not hidden because the resulting
//! glyph would collapse into the surrounding text.
//!
//! Thread ownership: pure data; called from the display-map worker
//! thread that owns the builder.

use std::ops::Range;

use crate::id::SourceByte;

/// Compute the document-absolute byte ranges to hide on the line
/// covering `[line_start, line_end)` so each recognised `\X` escape
/// renders as just `X`.
///
/// Returns an empty vector when the caret lies on this line — that's
/// the reveal range, mirroring how `block_revealed` / `line_revealed`
/// work for inline-marker reveal: editing the line shows raw bytes.
///
/// `line_text` is the line content without trailing newline, matching
/// the slice the display-map builder already has on hand.
#[must_use]
pub fn compute_backslash_hidden_ranges_for_line(
    caret_bytes: &[SourceByte],
    line_start: usize,
    line_end: usize,
    line_text: &str,
) -> Vec<Range<usize>> {
    if is_caret_on_line(caret_bytes, line_start, line_end) {
        return Vec::new();
    }
    let mut out: Vec<Range<usize>> = Vec::new();
    let bytes = line_text.as_bytes();
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] == b'\\' && is_escapable(bytes[i + 1]) {
            // Hide only the backslash byte — the follow-on character
            // is part of the visible run.
            out.push((line_start + i)..(line_start + i + 1));
            // Skip both bytes so a literal `\\X` keeps the second
            // backslash + the X visible (consistent with markdown
            // double-escape).
            i += 2;
            continue;
        }
        i += 1;
    }
    out
}

fn is_caret_on_line(caret_bytes: &[SourceByte], line_start: usize, line_end: usize) -> bool {
    caret_bytes.iter().any(|c| {
        let b = c.as_usize();
        b >= line_start && b <= line_end
    })
}

/// CommonMark §6.1 backslash-escape set, ASCII-punctuation only. Space
/// and tab are intentionally excluded — collapsing them would silently
/// alter prose layout. Letters and digits are excluded because `\a` /
/// `\1` are not escape sequences in markdown.
fn is_escapable(byte: u8) -> bool {
    matches!(
        byte,
        b'!' | b'"'
            | b'#'
            | b'$'
            | b'%'
            | b'&'
            | b'\''
            | b'('
            | b')'
            | b'*'
            | b'+'
            | b','
            | b'-'
            | b'.'
            | b'/'
            | b':'
            | b';'
            | b'<'
            | b'='
            | b'>'
            | b'?'
            | b'@'
            | b'['
            | b'\\'
            | b']'
            | b'^'
            | b'_'
            | b'`'
            | b'{'
            | b'|'
            | b'}'
            | b'~'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caret(b: usize) -> SourceByte {
        SourceByte::from_usize(b)
    }

    #[test]
    fn caret_on_line_disables_hiding() {
        // Caret inside the line: every `\X` stays visible so the user
        // can see what they typed.
        let line = r"\*literal\*";
        let out = compute_backslash_hidden_ranges_for_line(&[caret(3)], 0, line.len(), line);
        assert!(out.is_empty());
    }

    #[test]
    fn caret_off_line_hides_each_leading_backslash() {
        let line = r"\*literal\*";
        let out = compute_backslash_hidden_ranges_for_line(&[caret(100)], 0, line.len(), line);
        // Two backslashes at offsets 0 and 9.
        assert_eq!(out, vec![0..1, 9..10]);
    }

    #[test]
    fn double_backslash_hides_only_the_first() {
        // `\\*` in source → first `\` is the escape for the second `\`,
        // so the second `\` is the *visible* literal. The leading
        // backslash gets hidden, the following `\*` stays.
        let line = r"\\*";
        let out = compute_backslash_hidden_ranges_for_line(&[caret(100)], 0, line.len(), line);
        assert_eq!(out, vec![0..1]);
    }

    #[test]
    fn non_escapable_target_leaves_backslash_visible() {
        // `\a` is not a markdown escape — show the backslash so the
        // user knows their text is literally `\a`.
        let line = r"\a";
        let out = compute_backslash_hidden_ranges_for_line(&[caret(100)], 0, line.len(), line);
        assert!(out.is_empty());
    }

    #[test]
    fn trailing_solo_backslash_left_alone() {
        // A backslash at the end of a line with no following char is a
        // literal, not an escape.
        let line = r"end\";
        let out = compute_backslash_hidden_ranges_for_line(&[caret(100)], 0, line.len(), line);
        assert!(out.is_empty());
    }

    #[test]
    fn ranges_carry_line_start_offset() {
        // Same content but the line lives 50 bytes into the document.
        let line = r"a\*b";
        let out = compute_backslash_hidden_ranges_for_line(&[caret(0)], 50, 50 + line.len(), line);
        assert_eq!(out, vec![51..52]);
    }

    #[test]
    fn does_not_hide_escaped_whitespace() {
        // `\<space>` and `\<tab>` are excluded — hiding them would
        // collapse surrounding glyphs.
        let line = "a\\ b\\\tc";
        let out =
            compute_backslash_hidden_ranges_for_line(&[caret(0)], 100, 100 + line.len(), line);
        assert!(out.is_empty());
    }
}
