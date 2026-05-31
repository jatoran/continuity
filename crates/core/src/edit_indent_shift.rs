//! Position-shift helpers for legacy line-spanning indent/outdent.
//!
//! Extracted from [`crate::edit_line_text`] so that file stays under
//! the 600-line cap. The planners in `edit_line_text.rs` use these to
//! compute correct `selections_after` lists when a multi-line range
//! gets indented or outdented — without them, the legacy clone-of-
//! `selections_before` left the post-edit selection clinging to byte
//! offsets that no longer exist in the new rope (manifesting as a
//! "Shift+Tab made my selection vanish" bug).

use continuity_text::Position;

use crate::selection_edit::IndentUnit;

fn leading_run<F: Fn(char) -> bool>(s: &str, predicate: F) -> usize {
    s.chars()
        .take_while(|c| predicate(*c))
        .map(char::len_utf8)
        .sum()
}

/// Number of leading bytes plan_outdent should delete from a line slice
/// to outdent by one [`IndentUnit`]. Mirrors the historical inline
/// expression; lives here so both legacy + Phase-B10 caret-only branches
/// share a single source of truth.
pub(crate) fn outdent_drop_len(slice: &str, unit: IndentUnit) -> usize {
    match unit {
        IndentUnit::Tab => {
            if slice.starts_with('\t') {
                1
            } else {
                leading_run(slice, |c| c == ' ').min(8)
            }
        }
        IndentUnit::Spaces(n) => {
            let n = n as usize;
            let leading = leading_run(slice, |c| c == ' ');
            if leading >= n {
                n
            } else if slice.starts_with('\t') {
                1
            } else {
                leading
            }
        }
    }
}

/// Shift `p`'s byte_in_line by `prefix_len` when its source line was
/// indented; passthrough otherwise.
pub(crate) fn shift_after_indent(
    p: Position,
    indented_lines: &[usize],
    prefix_len: u32,
) -> Position {
    if indented_lines.contains(&(p.line as usize)) {
        Position::new(p.line, p.byte_in_line + prefix_len)
    } else {
        p
    }
}

/// Subtract the per-line `drop_len` from positions on outdented lines.
/// Positions inside the dropped run collapse to column 0.
pub(crate) fn shift_after_outdent(p: Position, drops: &[(usize, u32)]) -> Position {
    let line = p.line as usize;
    let Some(&(_, drop_len)) = drops.iter().find(|(l, _)| *l == line) else {
        return p;
    };
    if p.byte_in_line > drop_len {
        Position::new(p.line, p.byte_in_line - drop_len)
    } else {
        Position::new(p.line, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indent_shift_only_touches_indented_lines() {
        let p = shift_after_indent(Position::new(1, 3), &[1], 2);
        assert_eq!(p, Position::new(1, 5));
        let p = shift_after_indent(Position::new(2, 3), &[1], 2);
        assert_eq!(p, Position::new(2, 3));
    }

    #[test]
    fn outdent_shift_clamps_inside_dropped_run() {
        let drops = &[(0_usize, 4_u32)];
        assert_eq!(
            shift_after_outdent(Position::new(0, 6), drops),
            Position::new(0, 2)
        );
        assert_eq!(
            shift_after_outdent(Position::new(0, 3), drops),
            Position::new(0, 0)
        );
        assert_eq!(
            shift_after_outdent(Position::new(0, 4), drops),
            Position::new(0, 0)
        );
        assert_eq!(
            shift_after_outdent(Position::new(1, 6), drops),
            Position::new(1, 6)
        );
    }
}
