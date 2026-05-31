//! Line-text edits — sort, reverse, unique, shuffle, trim trailing
//! whitespace, indent/outdent, tabs↔spaces, and line-ending conversion.
//!
//! These operate on the line block covered by selections and rewrite the
//! covered lines as a single deterministic replacement (one undo group per
//! call). Line-structure ops (newline insertion, duplicate, move, join) live
//! in `edit_lines.rs`.

use continuity_buffer::Buffer;

use crate::edit_line_text_helpers::{indent_text, sort_in_place};
use crate::edit_lines::{lines_covered, rewrite_covered_lines};
use crate::edit_planning::{finalize_specs, line_content_end, EditSpec};
use crate::selection_edit::SelectionEditPlan;
use crate::Error;
use crate::{IndentUnit, LineEnding, SortKind};

pub(crate) fn plan_sort_lines(
    buffer: &Buffer,
    kind: SortKind,
) -> Result<Option<SelectionEditPlan>, Error> {
    rewrite_covered_lines(buffer, |lines| {
        let mut sorted = lines;
        sort_in_place(&mut sorted, kind);
        sorted
    })
}

pub(crate) fn plan_reverse_lines(buffer: &Buffer) -> Result<Option<SelectionEditPlan>, Error> {
    rewrite_covered_lines(buffer, |lines| {
        let mut rev = lines;
        rev.reverse();
        rev
    })
}

pub(crate) fn plan_unique_lines(buffer: &Buffer) -> Result<Option<SelectionEditPlan>, Error> {
    rewrite_covered_lines(buffer, |lines| {
        let mut seen = std::collections::HashSet::new();
        lines
            .into_iter()
            .filter(|l| seen.insert(l.clone()))
            .collect()
    })
}

pub(crate) fn plan_shuffle_lines(
    buffer: &Buffer,
    seed: u64,
) -> Result<Option<SelectionEditPlan>, Error> {
    rewrite_covered_lines(buffer, |mut lines| {
        let mut state = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        for i in (1..lines.len()).rev() {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let j = (state >> 33) as usize % (i + 1);
            lines.swap(i, j);
        }
        lines
    })
}

pub(crate) fn plan_trim_trailing_whitespace(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    let lines = lines_covered(buffer);
    plan_trim_lines(buffer, &lines)
}

/// Phase B14 — trim trailing whitespace on every line in the buffer.
pub(crate) fn plan_trim_trailing_whitespace_all(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    let total = buffer.rope().len_lines();
    let lines: Vec<usize> = (0..total).collect();
    plan_trim_lines(buffer, &lines)
}

fn plan_trim_lines(buffer: &Buffer, lines: &[usize]) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let mut specs = Vec::new();
    for &line in lines {
        let start = rope.line_to_byte(line);
        let end = line_content_end(rope, line);
        let slice = rope.byte_slice(start..end).to_string();
        let trimmed = slice.trim_end_matches([' ', '\t']);
        if trimmed.len() < slice.len() {
            specs.push(EditSpec::delete(rope, start + trimmed.len(), end)?);
        }
    }
    Ok(finalize_specs(
        specs,
        selections_before.clone(),
        selections_before,
    ))
}

pub(crate) fn plan_indent(
    buffer: &Buffer,
    unit: IndentUnit,
) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let prefix = indent_text(unit);
    let selections_before = buffer.selections().to_vec();

    // When every selection is a collapsed caret, Tab inserts the indent
    // text at the caret(s) — matching what every other editor does. The
    // line-prefix branch below only kicks in when the user has actually
    // selected text spanning one or more lines.
    let all_caret = !selections_before.is_empty() && selections_before.iter().all(|s| s.is_caret());
    if all_caret {
        let mut specs = Vec::new();
        let mut selections_after = Vec::new();
        for selection in &selections_before {
            // Phase B10: a caret on a list-item line indents the
            // *line*, not the caret, and shifts the caret right by the
            // indent unit so its position within the line is
            // preserved. Falls through to the legacy
            // insert-at-caret behaviour for non-list lines.
            let line = selection.head.line as usize;
            let line_start = rope.line_to_byte(line);
            let line_end = crate::edit_planning::line_content_end(rope, line);
            let line_text = rope.byte_slice(line_start..line_end).to_string();
            let leading_len = line_text
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .map(char::len_utf8)
                .sum::<usize>();
            let body = &line_text[leading_len..];
            if crate::edit_list::detect_list_marker(body).is_some() {
                specs.push(EditSpec::insert(rope, line_start, prefix.clone())?);
                let new_head = continuity_text::Position::new(
                    selection.head.line,
                    selection.head.byte_in_line + prefix.len() as u32,
                );
                selections_after.push(continuity_text::Selection::caret_at(new_head));
                continue;
            }
            let byte = selection.head.to_byte_offset(rope)?;
            specs.push(EditSpec::insert(rope, byte, prefix.clone())?);
            let new_head = crate::edit_planning::advance_position(selection.head, &prefix);
            selections_after.push(continuity_text::Selection::caret_at(new_head));
        }
        return Ok(finalize_specs(specs, selections_before, selections_after));
    }

    let lines = lines_covered(buffer);
    let mut specs = Vec::new();
    for &line in &lines {
        let start = rope.line_to_byte(line);
        specs.push(EditSpec::insert(rope, start, prefix.clone())?);
    }
    // Phase-bugfix: shift each selection's byte_in_line by `prefix.len()`
    // on every line that received an indent prefix, so the post-edit
    // selection still highlights the *same* visible characters (the
    // legacy `selections_before` clone left the selection visually
    // anchored to the wrong byte offsets in the new rope).
    let prefix_len = prefix.len() as u32;
    let selections_after: Vec<continuity_text::Selection> = selections_before
        .iter()
        .map(|sel| continuity_text::Selection {
            anchor: shift_after_indent(sel.anchor, &lines, prefix_len),
            head: shift_after_indent(sel.head, &lines, prefix_len),
            kind: sel.kind,
        })
        .collect();
    Ok(finalize_specs(specs, selections_before, selections_after))
}

use crate::edit_indent_shift::{shift_after_indent, shift_after_outdent};

pub(crate) fn plan_outdent(
    buffer: &Buffer,
    unit: IndentUnit,
) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    // Phase B10: caret-only outdent on a list line outdents the line
    // and shifts the caret left by the dropped run, preserving its
    // content offset. Falls through for everything else.
    let all_caret = !selections_before.is_empty() && selections_before.iter().all(|s| s.is_caret());
    if all_caret {
        let mut specs = Vec::new();
        let mut selections_after = Vec::new();
        let mut any_list = false;
        for selection in &selections_before {
            let line = selection.head.line as usize;
            let line_start = rope.line_to_byte(line);
            let line_end = crate::edit_planning::line_content_end(rope, line);
            let line_text = rope.byte_slice(line_start..line_end).to_string();
            let leading_len = line_text
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .map(char::len_utf8)
                .sum::<usize>();
            let body = &line_text[leading_len..];
            if crate::edit_list::detect_list_marker(body).is_some() {
                any_list = true;
                let drop_len = outdent_drop_len(&line_text, unit);
                if drop_len > 0 {
                    specs.push(EditSpec::delete(rope, line_start, line_start + drop_len)?);
                    let new_col =
                        (selection.head.byte_in_line as usize).saturating_sub(drop_len) as u32;
                    selections_after.push(continuity_text::Selection::caret_at(
                        continuity_text::Position::new(selection.head.line, new_col),
                    ));
                } else {
                    selections_after.push(*selection);
                }
            }
        }
        if any_list {
            return Ok(finalize_specs(specs, selections_before, selections_after));
        }
    }
    let lines = lines_covered(buffer);
    let mut specs = Vec::new();
    // line -> drop_len map so post-edit selection shifting can subtract
    // exactly what was deleted on each affected line.
    let mut drops: Vec<(usize, u32)> = Vec::new();
    for &line in &lines {
        let start = rope.line_to_byte(line);
        let end = line_content_end(rope, line);
        let slice = rope.byte_slice(start..end).to_string();
        let drop_len = outdent_drop_len(&slice, unit);
        if drop_len > 0 {
            specs.push(EditSpec::delete(rope, start, start + drop_len)?);
            drops.push((line, drop_len as u32));
        }
    }
    let selections_after: Vec<continuity_text::Selection> = selections_before
        .iter()
        .map(|sel| continuity_text::Selection {
            anchor: shift_after_outdent(sel.anchor, &drops),
            head: shift_after_outdent(sel.head, &drops),
            kind: sel.kind,
        })
        .collect();
    Ok(finalize_specs(specs, selections_before, selections_after))
}

pub(crate) fn plan_spaces_to_tabs(
    buffer: &Buffer,
    width: u32,
) -> Result<Option<SelectionEditPlan>, Error> {
    let width = width.max(1) as usize;
    let space_run: String = " ".repeat(width);
    rewrite_covered_lines(buffer, |lines| {
        lines
            .into_iter()
            .map(|line| line.replace(&space_run, "\t"))
            .collect()
    })
}

pub(crate) fn plan_tabs_to_spaces(
    buffer: &Buffer,
    width: u32,
) -> Result<Option<SelectionEditPlan>, Error> {
    let spaces: String = " ".repeat(width as usize);
    rewrite_covered_lines(buffer, |lines| {
        lines
            .into_iter()
            .map(|line| line.replace('\t', &spaces))
            .collect()
    })
}

pub(crate) fn plan_convert_line_endings(
    buffer: &Buffer,
    eol: LineEnding,
) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let target = match eol {
        LineEnding::Lf => "\n",
        LineEnding::Crlf => "\r\n",
    };
    let lines = lines_covered(buffer);
    let mut specs = Vec::new();
    for &line in &lines {
        if line + 1 >= rope.len_lines() {
            continue;
        }
        let content_end = line_content_end(rope, line);
        let next_start = rope.line_to_byte(line + 1);
        if next_start <= content_end {
            continue;
        }
        let current = rope.byte_slice(content_end..next_start).to_string();
        if current != target {
            specs.push(EditSpec::replace(
                rope,
                content_end,
                next_start,
                target.to_string(),
            )?);
        }
    }
    Ok(finalize_specs(
        specs,
        selections_before.clone(),
        selections_before,
    ))
}

use crate::edit_indent_shift::outdent_drop_len;

#[cfg(test)]
mod tests {
    use continuity_buffer::Buffer;
    use continuity_text::{Position, Selection};

    use super::*;
    use crate::selection_edit::{apply_plan, plan, SelectionEdit};

    fn at(line: u32, col: u32) -> Selection {
        Selection::caret_at(Position::new(line, col))
    }

    fn build(text: &str, selections: Vec<Selection>) -> Buffer {
        let mut buffer = Buffer::from_text(text);
        buffer.set_selections(selections);
        buffer
    }

    fn run(buffer: &mut Buffer, edit: SelectionEdit) {
        let plan = plan(buffer, &edit).expect("plan ok").expect("plan some");
        apply_plan(buffer, &plan).expect("apply ok");
    }

    #[test]
    fn sort_lines_ascending() {
        let mut b = Buffer::from_text("c\nb\na");
        b.set_selections(vec![Selection::new(
            Position::new(0, 0),
            Position::new(2, 1),
            continuity_text::SelectionKind::Caret,
        )]);
        run(&mut b, SelectionEdit::SortLines(SortKind::Asc));
        assert_eq!(b.rope().to_string(), "a\nb\nc");
    }

    #[test]
    fn indent_list_line_caret_shifts_preserves_offset() {
        // Phase B10 — Tab on a list line with caret in the body
        // prepends indent at line start and shifts caret right by the
        // indent unit, so the caret stays on the same content char.
        let mut b = Buffer::from_text("- foo");
        b.set_selections(vec![Selection::caret_at(Position::new(0, 4))]);
        run(
            &mut b,
            SelectionEdit::Indent {
                unit: IndentUnit::Spaces(2),
            },
        );
        assert_eq!(b.rope().to_string(), "  - foo");
        let head = b.selections()[0].head;
        assert_eq!(head.byte_in_line, 6);
    }

    #[test]
    fn indent_non_list_line_inserts_at_caret() {
        // Sanity: non-list line still takes the legacy insert-at-caret
        // path — Tab types two spaces wherever the caret sits.
        let mut b = Buffer::from_text("plain");
        b.set_selections(vec![Selection::caret_at(Position::new(0, 3))]);
        run(
            &mut b,
            SelectionEdit::Indent {
                unit: IndentUnit::Spaces(2),
            },
        );
        assert_eq!(b.rope().to_string(), "pla  in");
    }

    #[test]
    fn trim_trailing_whitespace_all_covers_every_line() {
        // Caret on line 0 — the all-variant strips trailing ws on
        // every line regardless of selection coverage.
        let mut b = Buffer::from_text("a   \nb\t\nc d  \n");
        b.set_selections(vec![Selection::caret_at(Position::new(0, 0))]);
        run(&mut b, SelectionEdit::TrimTrailingWhitespaceAll);
        assert_eq!(b.rope().to_string(), "a\nb\nc d\n");
    }

    #[test]
    fn indent_outdent_range_shifts_positions() {
        // Phase-bugfix: shift+tab / tab on a multi-line range shifts
        // positions correctly — selection survives over the same
        // visible characters.
        let mut b = Buffer::from_text("    abc\n    def\n    ghi");
        b.set_selections(vec![Selection::new(
            Position::new(0, 4),
            Position::new(2, 7),
            continuity_text::SelectionKind::Caret,
        )]);
        run(
            &mut b,
            SelectionEdit::Outdent {
                unit: IndentUnit::Spaces(4),
            },
        );
        assert_eq!(b.rope().to_string(), "abc\ndef\nghi");
        let s = b.selections()[0];
        assert_eq!(
            (s.anchor, s.head),
            (Position::new(0, 0), Position::new(2, 3))
        );
        let mut b = Buffer::from_text("abc\ndef");
        b.set_selections(vec![Selection::new(
            Position::new(0, 1),
            Position::new(1, 2),
            continuity_text::SelectionKind::Caret,
        )]);
        run(
            &mut b,
            SelectionEdit::Indent {
                unit: IndentUnit::Spaces(2),
            },
        );
        assert_eq!(b.rope().to_string(), "  abc\n  def");
        let s = b.selections()[0];
        assert_eq!(
            (s.anchor, s.head),
            (Position::new(0, 3), Position::new(1, 4))
        );
    }

    #[test]
    fn outdent_list_line_caret_shifts_left() {
        let mut b = Buffer::from_text("    - foo");
        b.set_selections(vec![Selection::caret_at(Position::new(0, 8))]);
        run(
            &mut b,
            SelectionEdit::Outdent {
                unit: IndentUnit::Spaces(2),
            },
        );
        assert_eq!(b.rope().to_string(), "  - foo");
        let head = b.selections()[0].head;
        assert_eq!(head.byte_in_line, 6);
    }

    #[test]
    fn reverse_lines_flips_block() {
        let mut b = Buffer::from_text("1\n2\n3");
        b.set_selections(vec![Selection::new(
            Position::new(0, 0),
            Position::new(2, 1),
            continuity_text::SelectionKind::Caret,
        )]);
        run(&mut b, SelectionEdit::ReverseLines);
        assert_eq!(b.rope().to_string(), "3\n2\n1");
    }

    #[test]
    fn unique_lines_drops_dupes() {
        let mut b = Buffer::from_text("a\nb\na");
        b.set_selections(vec![Selection::new(
            Position::new(0, 0),
            Position::new(2, 1),
            continuity_text::SelectionKind::Caret,
        )]);
        run(&mut b, SelectionEdit::UniqueLines);
        assert_eq!(b.rope().to_string(), "a\nb");
    }

    #[test]
    fn shuffle_lines_is_deterministic() {
        let make = || {
            let mut buf = Buffer::from_text("1\n2\n3\n4\n5");
            buf.set_selections(vec![Selection::new(
                Position::new(0, 0),
                Position::new(4, 1),
                continuity_text::SelectionKind::Caret,
            )]);
            buf
        };
        let mut a = make();
        let mut b = make();
        run(&mut a, SelectionEdit::ShuffleLines(7));
        run(&mut b, SelectionEdit::ShuffleLines(7));
        assert_eq!(a.rope().to_string(), b.rope().to_string());
    }

    #[test]
    fn trim_trailing_whitespace_only_trailing() {
        let mut b = build("foo  \nbar", vec![at(0, 0)]);
        run(&mut b, SelectionEdit::TrimTrailingWhitespace);
        assert_eq!(b.rope().to_string(), "foo\nbar");
    }

    #[test]
    fn indent_inserts_prefix() {
        let mut b = build("a", vec![at(0, 0)]);
        run(
            &mut b,
            SelectionEdit::Indent {
                unit: IndentUnit::Spaces(2),
            },
        );
        assert_eq!(b.rope().to_string(), "  a");
    }

    #[test]
    fn outdent_drops_prefix() {
        let mut b = build("    a", vec![at(0, 0)]);
        run(
            &mut b,
            SelectionEdit::Outdent {
                unit: IndentUnit::Spaces(2),
            },
        );
        assert_eq!(b.rope().to_string(), "  a");
    }

    #[test]
    fn spaces_to_tabs_converts_runs() {
        let mut b = build("    foo", vec![at(0, 0)]);
        run(&mut b, SelectionEdit::SpacesToTabs { tab_width: 4 });
        assert_eq!(b.rope().to_string(), "\tfoo");
    }

    #[test]
    fn tabs_to_spaces_converts() {
        let mut b = build("\tfoo", vec![at(0, 0)]);
        run(&mut b, SelectionEdit::TabsToSpaces { tab_width: 2 });
        assert_eq!(b.rope().to_string(), "  foo");
    }

    #[test]
    fn convert_line_endings_lf_to_crlf() {
        let mut b = Buffer::from_text("a\nb");
        b.set_selections(vec![Selection::new(
            Position::new(0, 0),
            Position::new(1, 1),
            continuity_text::SelectionKind::Caret,
        )]);
        run(&mut b, SelectionEdit::ConvertLineEndings(LineEnding::Crlf));
        assert_eq!(b.rope().to_string(), "a\r\nb");
    }
}
