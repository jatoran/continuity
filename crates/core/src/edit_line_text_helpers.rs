//! Pure helpers extracted from [`crate::edit_line_text`] to keep that
//! file under the 600-line cap (CLAUDE.md conventions).
//!
//! Two unrelated bundles live here:
//! - `indent_text` — render one indent unit (tab or N spaces) as a `String`.
//! - `sort_in_place` + `natural_cmp` + `take_digits` — the line-sort
//!   comparator family used by [`crate::selection_edit::SelectionEdit::SortLines`].
//!
//! Thread ownership: stateless, callable from any thread.

use crate::selection_edit::SortKind;
use crate::IndentUnit;

/// Render one indent unit as the literal text inserted at line start
/// when the user presses `Tab` on a list line / range.
pub(crate) fn indent_text(unit: IndentUnit) -> String {
    match unit {
        IndentUnit::Tab => "\t".to_string(),
        IndentUnit::Spaces(n) => " ".repeat(n as usize),
    }
}

/// Sort `lines` in-place according to `kind`. Stable ordering across
/// each variant.
pub(crate) fn sort_in_place(lines: &mut [String], kind: SortKind) {
    match kind {
        SortKind::Asc => lines.sort(),
        SortKind::Desc => {
            lines.sort();
            lines.reverse();
        }
        SortKind::AscCaseInsensitive => lines.sort_by_key(|s| s.to_ascii_lowercase()),
        SortKind::DescCaseInsensitive => {
            lines.sort_by_key(|s| std::cmp::Reverse(s.to_ascii_lowercase()))
        }
        SortKind::AscByLength => lines.sort_by_key(|s| s.chars().count()),
        SortKind::DescByLength => lines.sort_by_key(|s| std::cmp::Reverse(s.chars().count())),
        SortKind::AscNumeric => lines.sort_by(|a, b| natural_cmp(a, b)),
        SortKind::DescNumeric => lines.sort_by(|a, b| natural_cmp(b, a)),
    }
}

/// Natural ordering: digit runs compare numerically, everything else
/// compares by Unicode codepoint.
pub(crate) fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek(), bi.peek()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(ac), Some(bc)) => {
                if ac.is_ascii_digit() && bc.is_ascii_digit() {
                    let an = take_digits(&mut ai);
                    let bn = take_digits(&mut bi);
                    let cmp = an.cmp(&bn);
                    if cmp != std::cmp::Ordering::Equal {
                        return cmp;
                    }
                } else {
                    let cmp = ac.cmp(bc);
                    if cmp != std::cmp::Ordering::Equal {
                        return cmp;
                    }
                    ai.next();
                    bi.next();
                }
            }
        }
    }
}

fn take_digits<I: Iterator<Item = char>>(iter: &mut std::iter::Peekable<I>) -> u128 {
    let mut n: u128 = 0;
    while let Some(c) = iter.peek() {
        if let Some(d) = c.to_digit(10) {
            n = n.saturating_mul(10).saturating_add(u128::from(d));
            iter.next();
        } else {
            break;
        }
    }
    n
}
