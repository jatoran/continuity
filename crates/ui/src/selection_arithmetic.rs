//! G5 — selection arithmetic: keep / discard / split-on regex.
//!
//! Split out of `selection.rs` to keep that file under the 600-line
//! cap. The Window method `selection_arithmetic_impl` is the single
//! entry; it delegates to a typed `Op` enum so the three commands
//! share one regex compile + selection-walk path.

use continuity_text::{Position, Selection, SelectionKind};
use ropey::Rope;

use crate::selection::{dedupe, match_selection};
use crate::Window;

impl Window {
    /// G5 — apply selection arithmetic. `op` is `"keep"` / `"discard"`
    /// / `"split"`. Malformed regex or unknown op → `false` (no-op).
    pub(crate) fn selection_arithmetic_impl(&mut self, op: &str, pattern: &str) -> bool {
        let Ok(re) = continuity_search::compile_regex(pattern) else {
            return false;
        };
        self.map_selections_for_arithmetic(|rope, selections| {
            apply_arithmetic(rope, selections, op, &re)
        })
    }

    fn map_selections_for_arithmetic<F>(&mut self, f: F) -> bool
    where
        F: FnOnce(&Rope, &[Selection]) -> Vec<Selection>,
    {
        let Some(snapshot) = self.current_snapshot() else {
            return false;
        };
        let next = f(snapshot.rope_snapshot().rope(), snapshot.selections());
        self.editor.set_selections(self.buffer_id, next).is_ok()
    }
}

fn apply_arithmetic(
    rope: &Rope,
    selections: &[Selection],
    op: &str,
    re: &continuity_search::CompiledRegex,
) -> Vec<Selection> {
    match op {
        "keep" => selections
            .iter()
            .filter(|s| re.is_match(selection_text(rope, s).as_bytes()))
            .copied()
            .collect(),
        "discard" => selections
            .iter()
            .filter(|s| !re.is_match(selection_text(rope, s).as_bytes()))
            .copied()
            .collect(),
        "split" => split_each(rope, selections, re),
        _ => selections.to_vec(),
    }
}

fn split_each(
    rope: &Rope,
    selections: &[Selection],
    re: &continuity_search::CompiledRegex,
) -> Vec<Selection> {
    let mut out: Vec<Selection> = Vec::new();
    for s in selections {
        let txt = selection_text(rope, s);
        let start_byte = ordered_start_byte(rope, s);
        let bytes = txt.as_bytes();
        let ranges = re.find_ranges(bytes);
        if ranges.is_empty() {
            out.push(*s);
            continue;
        }
        let mut cursor = 0usize;
        for (mstart, mend) in ranges {
            if mstart > cursor {
                push_sub(rope, &mut out, start_byte + cursor, start_byte + mstart);
            }
            // Zero-width matches still advance — defends against
            // patterns like `(?=x)` looping forever.
            cursor = mend.max(cursor + 1).min(bytes.len());
        }
        if cursor < bytes.len() {
            push_sub(
                rope,
                &mut out,
                start_byte + cursor,
                start_byte + bytes.len(),
            );
        }
    }
    dedupe(out)
}

fn selection_text(rope: &Rope, s: &Selection) -> String {
    let r = s.ordered_range();
    let start = r.start.to_byte_offset(rope).unwrap_or(0);
    let end = r.end.to_byte_offset(rope).unwrap_or(start);
    rope.byte_slice(start..end).to_string()
}

fn ordered_start_byte(rope: &Rope, s: &Selection) -> usize {
    s.ordered_range().start.to_byte_offset(rope).unwrap_or(0)
}

fn push_sub(rope: &Rope, out: &mut Vec<Selection>, start: usize, end: usize) {
    if start == end {
        let p = Position::from_byte_offset(rope, start).unwrap_or(Position::ZERO);
        out.push(Selection::new(p, p, SelectionKind::Caret));
    } else {
        out.push(match_selection(rope, start, end - start));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(rope: &Rope, start: usize, end: usize) -> Selection {
        let a = Position::from_byte_offset(rope, start).unwrap();
        let h = Position::from_byte_offset(rope, end).unwrap();
        Selection::new(a, h, SelectionKind::Caret)
    }

    #[test]
    fn keep_drops_non_matching_selections() {
        let rope = Rope::from_str("foo bar baz");
        let sels = vec![sel(&rope, 0, 3), sel(&rope, 4, 7), sel(&rope, 8, 11)];
        let re = continuity_search::compile_regex("ba").unwrap();
        let out = apply_arithmetic(&rope, &sels, "keep", &re);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn discard_drops_matching_selections() {
        let rope = Rope::from_str("foo bar baz");
        let sels = vec![sel(&rope, 0, 3), sel(&rope, 4, 7), sel(&rope, 8, 11)];
        let re = continuity_search::compile_regex("ba").unwrap();
        let out = apply_arithmetic(&rope, &sels, "discard", &re);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn split_on_breaks_selection_at_matches() {
        let rope = Rope::from_str("a,b,c");
        let sels = vec![sel(&rope, 0, 5)];
        let re = continuity_search::compile_regex(",").unwrap();
        let out = apply_arithmetic(&rope, &sels, "split", &re);
        // Three sub-selections: "a", "b", "c".
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn split_on_no_match_keeps_original() {
        let rope = Rope::from_str("abc");
        let sels = vec![sel(&rope, 0, 3)];
        let re = continuity_search::compile_regex(",").unwrap();
        let out = apply_arithmetic(&rope, &sels, "split", &re);
        assert_eq!(out, sels);
    }

    #[test]
    fn unknown_op_is_identity() {
        let rope = Rope::from_str("abc");
        let sels = vec![sel(&rope, 0, 3)];
        let re = continuity_search::compile_regex("a").unwrap();
        let out = apply_arithmetic(&rope, &sels, "frobnicate", &re);
        assert_eq!(out, sels);
    }
}
