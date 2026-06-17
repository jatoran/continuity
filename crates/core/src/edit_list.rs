//! Phase B9 / B10 / B11 list-aware editing helpers.
//!
//! Markdown list markers (`-`, `*`, `+`, `N.`) get smart-newline,
//! indent, and renumber behaviour. The detection lives in this file
//! so [`crate::edit_lines`] stays under the 600-line cap and the
//! tests are tight.
//!
//! The helpers here are pure — they operate on `&str` line bodies —
//! so they can be exercised without spinning up a buffer. Higher-level
//! planners in `edit_lines.rs` consume them when assembling
//! [`crate::SelectionEditPlan`]s.

/// Detected list marker at the head of a line's body (post leading
/// whitespace).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ListMarker {
    /// Marker glyph for unordered lists (`'-' | '*' | '+'`) or the
    /// dot character `'.'` for ordered. The actual rendering is
    /// reconstructed via [`Self::as_str`] / [`Self::next_marker`].
    pub kind: ListMarkerKind,
    /// Byte length of the marker prefix in the original source,
    /// including the trailing space (`"- "` → 2, `"123. "` → 5).
    pub prefix_len: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ListMarkerKind {
    /// Unordered marker glyph.
    Unordered(char),
    /// Ordered marker with the captured integer.
    Ordered(u32),
}

impl ListMarker {
    /// Construct the marker for the *next* list item: unordered keeps
    /// the same glyph; ordered increments the number.
    pub(crate) fn next_marker(&self) -> String {
        match self.kind {
            ListMarkerKind::Unordered(c) => format!("{c} "),
            ListMarkerKind::Ordered(n) => format!("{}. ", n.saturating_add(1)),
        }
    }
}

/// Detect a list marker at the start of `body` (which is the line
/// content with leading whitespace already stripped). Returns the
/// marker shape + prefix byte length, or `None` when the line does
/// not begin with a recognised list marker.
pub(crate) fn detect_list_marker(body: &str) -> Option<ListMarker> {
    // Unordered: '-', '*', '+' followed by ' '.
    if let Some(rest) = body
        .strip_prefix("- ")
        .or_else(|| body.strip_prefix("* "))
        .or_else(|| body.strip_prefix("+ "))
    {
        let _ = rest;
        let glyph = body.chars().next().expect("non-empty body");
        return Some(ListMarker {
            kind: ListMarkerKind::Unordered(glyph),
            prefix_len: 2,
        });
    }
    // Ordered: one-or-more digits, then '.', then ' '.
    let digits: String = body.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let after_digits = &body[digits.len()..];
    if !after_digits.starts_with(". ") {
        return None;
    }
    let n: u32 = digits.parse().ok()?;
    Some(ListMarker {
        kind: ListMarkerKind::Ordered(n),
        prefix_len: digits.len() + 2,
    })
}

/// Decision the smart-newline planner needs: continue the list, end
/// the list, or fall through to ordinary indent-only newline.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ListNewlineAction {
    /// Insert `\n` + indent + the next marker.
    Continue {
        /// The next marker, e.g. `"2. "` or `"- "`.
        next_marker: String,
    },
    /// The line was a marker-only stub — clear it to plain whitespace
    /// of one-less indent level and leave the caret on the same line.
    End {
        /// New body (typically empty for a single-level list, or a
        /// reduced-indent string for a nested list).
        replacement: String,
    },
    /// No marker — fall through to plain smart-newline (indent only).
    None,
}

/// Decide what should happen when the user hits `Enter` on a line
/// with the given `leading_whitespace` and `body` (the substring
/// after the leading whitespace).
pub(crate) fn list_newline_action(
    leading_whitespace: &str,
    body: &str,
    indent_unit: &str,
) -> ListNewlineAction {
    let Some(marker) = detect_list_marker(body) else {
        return ListNewlineAction::None;
    };
    let content_after_marker = &body[marker.prefix_len..];
    // A task / checkbox line (`- [ ] foo` / `- [x] foo`) continues with
    // an *unchecked* box on the next line so writers keep ticking off
    // items without re-typing the `[ ] `. An empty task stub
    // (`- [ ] ` with no content after the box) ends the list, mirroring
    // the empty-bullet behaviour below. Detection reuses
    // [`crate::edit_markdown::split_leading_list_marker`] so the marker
    // glyph (`- ` / `* ` / `+ ` / `N. `) is split off before the box is
    // inspected.
    let (_, after_marker) = crate::edit_markdown::split_leading_list_marker(body);
    if let Some(task_content) = after_marker
        .strip_prefix("[ ] ")
        .or_else(|| after_marker.strip_prefix("[x] "))
        .or_else(|| after_marker.strip_prefix("[X] "))
    {
        if task_content.is_empty() {
            // Empty task stub: dedent one level, same as an empty
            // bullet — strip exactly one `indent_unit` of leading
            // whitespace if present, else clear the line.
            let replacement = leading_whitespace
                .strip_prefix(indent_unit)
                .map_or(String::new(), str::to_string);
            return ListNewlineAction::End { replacement };
        }
        return ListNewlineAction::Continue {
            next_marker: format!("{}[ ] ", marker.next_marker()),
        };
    }
    if content_after_marker.is_empty() {
        // Empty list item: dedent one level. If leading whitespace
        // starts with `indent_unit`, strip exactly that prefix —
        // otherwise clear the line entirely.
        let replacement = leading_whitespace
            .strip_prefix(indent_unit)
            .map_or(String::new(), str::to_string);
        return ListNewlineAction::End { replacement };
    }
    ListNewlineAction::Continue {
        next_marker: marker.next_marker(),
    }
}

use continuity_buffer::Buffer;
use continuity_text::{Position, Selection};

use crate::edit_planning::{advance_position, finalize_specs, line_content_end, EditSpec};
use crate::selection_edit::SelectionEditPlan;
use crate::Error;

/// Phase B9 list-aware smart newline. Splits per-selection logic out
/// of [`crate::edit_lines`] so that file stays under the 600-line cap.
pub(crate) fn plan_insert_newline_smart_list_aware(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();

    // Phase: auto-renumber. A single caret continuing an ordered list
    // takes a dedicated path that also rewrites the run's markers in the
    // same undo group, so `1.\n2.\n3.` with the caret at the end of `2.`
    // produces `1.\n2.\n3.\n4.` instead of `1.\n2.\n3.\n3.`. The
    // multi-cursor / non-ordered cases keep the simpler per-selection
    // behaviour below.
    if selections_before.len() == 1 {
        let selection = selections_before[0];
        if let Some(plan) = renumber::try_ordered_continue_with_renumber(rope, selection)? {
            return Ok(Some(plan));
        }
    }

    let mut specs = Vec::new();
    let mut selections_after = Vec::new();
    for selection in &selections_before {
        let head = selection.head;
        let byte = head.to_byte_offset(rope)?;
        let line = head.line as usize;
        let line_start = rope.line_to_byte(line);
        let line_end = line_content_end(rope, line);
        let line_text = rope.byte_slice(line_start..line_end).to_string();
        let indent = crate::edit_lines::leading_whitespace_for(rope, line);
        let body = &line_text[indent.len()..];

        match list_newline_action(&indent, body, "  ") {
            ListNewlineAction::Continue { next_marker } => {
                let inserted = format!("\n{indent}{next_marker}");
                specs.push(EditSpec::insert(rope, byte, inserted.clone())?);
                selections_after.push(Selection::caret_at(advance_position(head, &inserted)));
            }
            ListNewlineAction::End { replacement } => {
                specs.push(EditSpec::replace(
                    rope,
                    line_start,
                    line_end,
                    replacement.clone(),
                )?);
                selections_after.push(Selection::caret_at(Position::new(
                    line as u32,
                    replacement.len() as u32,
                )));
            }
            ListNewlineAction::None => {
                let inserted = format!("\n{indent}");
                specs.push(EditSpec::insert(rope, byte, inserted.clone())?);
                selections_after.push(Selection::caret_at(advance_position(head, &inserted)));
            }
        }
    }
    Ok(finalize_specs(specs, selections_before, selections_after))
}

// Phase B11 ordered-list renumbering + the smart-newline ordered-continue
// path live in `edit_list/renumber.rs` so this file stays under the
// 600-line cap. Re-exported so `selection_edit.rs` keeps its import paths.
mod renumber;
pub(crate) use renumber::{is_ordered_marker, ordered_list_range_for, plan_markdown_renumber_list};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_unordered_dash() {
        let m = detect_list_marker("- foo").expect("marker");
        assert_eq!(m.kind, ListMarkerKind::Unordered('-'));
        assert_eq!(m.prefix_len, 2);
        assert_eq!(m.next_marker(), "- ");
    }

    #[test]
    fn detects_unordered_asterisk_plus() {
        assert!(detect_list_marker("* foo").is_some());
        assert!(detect_list_marker("+ foo").is_some());
    }

    #[test]
    fn detects_ordered_with_increment() {
        let m = detect_list_marker("12. foo").expect("marker");
        assert_eq!(m.kind, ListMarkerKind::Ordered(12));
        assert_eq!(m.prefix_len, 4);
        assert_eq!(m.next_marker(), "13. ");
    }

    #[test]
    fn rejects_non_list_lines() {
        assert!(detect_list_marker("-foo").is_none());
        assert!(detect_list_marker(".something").is_none());
        assert!(detect_list_marker("12.foo").is_none());
        assert!(detect_list_marker("").is_none());
        assert!(detect_list_marker("plain text").is_none());
    }

    #[test]
    fn continues_list_with_content() {
        let action = list_newline_action("", "- foo", "  ");
        assert_eq!(
            action,
            ListNewlineAction::Continue {
                next_marker: "- ".into()
            }
        );
    }

    #[test]
    fn continues_ordered_list_with_increment() {
        let action = list_newline_action("", "3. foo", "  ");
        assert_eq!(
            action,
            ListNewlineAction::Continue {
                next_marker: "4. ".into()
            }
        );
    }

    #[test]
    fn ends_list_on_empty_marker() {
        let action = list_newline_action("", "- ", "  ");
        assert_eq!(
            action,
            ListNewlineAction::End {
                replacement: String::new()
            }
        );
    }

    #[test]
    fn empty_marker_in_nested_list_dedents_one_level() {
        let action = list_newline_action("    ", "- ", "  ");
        assert_eq!(
            action,
            ListNewlineAction::End {
                replacement: "  ".into()
            }
        );
    }

    #[test]
    fn empty_marker_at_root_clears_line() {
        let action = list_newline_action("", "- ", "    ");
        assert_eq!(
            action,
            ListNewlineAction::End {
                replacement: String::new()
            }
        );
    }

    #[test]
    fn non_list_line_returns_none_action() {
        let action = list_newline_action("", "plain prose", "  ");
        assert_eq!(action, ListNewlineAction::None);
    }

    #[test]
    fn empty_ordered_marker_dedents() {
        let action = list_newline_action("  ", "5. ", "  ");
        assert_eq!(
            action,
            ListNewlineAction::End {
                replacement: String::new()
            }
        );
    }

    use continuity_buffer::Buffer;
    use continuity_text::{Position, Selection};

    use crate::selection_edit::{apply_plan, plan, SelectionEdit};

    fn build_buf(text: &str, caret_line: u32, caret_col: u32) -> Buffer {
        let mut b = Buffer::from_text(text);
        b.set_selections(vec![Selection::caret_at(Position::new(
            caret_line, caret_col,
        ))]);
        b
    }

    fn run_renumber(b: &mut Buffer) {
        if let Some(p) = plan(b, &SelectionEdit::MarkdownRenumberList).expect("plan ok") {
            apply_plan(b, &p).expect("apply ok");
        }
    }

    #[test]
    fn renumbers_simple_ordered_list_from_one() {
        let mut b = build_buf("3. a\n5. b\n7. c", 0, 0);
        run_renumber(&mut b);
        assert_eq!(b.rope().to_string(), "1. a\n2. b\n3. c");
    }

    #[test]
    fn renumber_caret_in_middle_walks_both_ways() {
        let mut b = build_buf("9. a\n9. b\n9. c", 1, 0);
        run_renumber(&mut b);
        assert_eq!(b.rope().to_string(), "1. a\n2. b\n3. c");
    }

    #[test]
    fn renumber_stops_at_non_ordered_line() {
        let mut b = build_buf("5. a\n5. b\nplain\n5. c", 0, 0);
        run_renumber(&mut b);
        assert_eq!(b.rope().to_string(), "1. a\n2. b\nplain\n5. c");
    }

    #[test]
    fn renumber_does_not_touch_unordered_list() {
        let mut b = build_buf("- a\n- b", 0, 0);
        run_renumber(&mut b);
        assert_eq!(b.rope().to_string(), "- a\n- b");
    }

    #[test]
    fn renumber_excludes_nested_indent_levels() {
        // Outer list at indent 0 is renumbered. The nested item at
        // indent 4 is part of a different ordered run and stays as-is.
        let mut b = build_buf("3. a\n    7. nested\n5. b", 0, 0);
        run_renumber(&mut b);
        assert_eq!(b.rope().to_string(), "1. a\n    7. nested\n2. b");
    }

    #[test]
    fn renumber_noop_when_caret_off_list() {
        let mut b = build_buf("plain prose", 0, 0);
        run_renumber(&mut b);
        assert_eq!(b.rope().to_string(), "plain prose");
    }
}
