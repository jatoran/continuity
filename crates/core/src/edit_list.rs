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
use ropey::Rope;

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

/// Phase B11 — renumber the ordered list containing each caret.
///
/// "Ordered list" = a contiguous run of lines that all start (after
/// the same leading whitespace) with `N. ` markers. Walks upward and
/// downward from each caret's line, collects the run, and rewrites
/// markers as `1.`, `2.`, `3.`, … preserving each line's leading
/// whitespace + body. Lines whose leading whitespace differs from the
/// caret line are excluded (nested ordered lists are renumbered on
/// their own caret pass).
pub(crate) fn plan_markdown_renumber_list(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let mut specs = Vec::new();
    let mut visited: ahash::AHashSet<u32> = ahash::AHashSet::new();
    for selection in &selections_before {
        let caret_line = selection.head.line as usize;
        let Some((start_line, end_line, indent)) = ordered_list_range(rope, caret_line) else {
            continue;
        };
        if visited.contains(&(start_line as u32)) {
            continue;
        }
        visited.insert(start_line as u32);
        let mut counter: u32 = 1;
        for line in start_line..=end_line {
            let line_start = rope.line_to_byte(line);
            let line_end = line_content_end(rope, line);
            let line_text = rope.byte_slice(line_start..line_end).to_string();
            let body = &line_text[indent.len()..];
            let Some(marker) = detect_list_marker(body) else {
                continue;
            };
            let after_marker = &body[marker.prefix_len..];
            let new_line = format!("{indent}{counter}. {after_marker}");
            if new_line != line_text {
                specs.push(EditSpec::replace(rope, line_start, line_end, new_line)?);
            }
            counter = counter.saturating_add(1);
        }
    }
    Ok(finalize_specs(
        specs,
        selections_before.clone(),
        selections_before,
    ))
}

/// Walk up + down from `caret_line` while consecutive lines are
/// ordered-list items at the same leading-whitespace level.
/// Returns `(start, end, indent)` where `start..=end` is inclusive
/// and `indent` is the shared leading-whitespace string.
fn ordered_list_range(rope: &Rope, caret_line: usize) -> Option<(usize, usize, String)> {
    let total = rope.len_lines();
    if total == 0 || caret_line >= total {
        return None;
    }
    let indent = caret_line_indent_for(rope, caret_line);
    let body = line_body(rope, caret_line, &indent);
    let marker = detect_list_marker(&body)?;
    if !matches!(marker.kind, ListMarkerKind::Ordered(_)) {
        return None;
    }
    let mut start = caret_line;
    while start > 0 && is_ordered_at_indent(rope, start - 1, &indent) {
        start -= 1;
    }
    let mut end = caret_line;
    while end + 1 < total && is_ordered_at_indent(rope, end + 1, &indent) {
        end += 1;
    }
    Some((start, end, indent))
}

fn caret_line_indent_for(rope: &Rope, line: usize) -> String {
    let start = rope.line_to_byte(line);
    let end = line_content_end(rope, line);
    let text = rope.byte_slice(start..end).to_string();
    text.chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

fn line_body(rope: &Rope, line: usize, indent: &str) -> String {
    let start = rope.line_to_byte(line);
    let end = line_content_end(rope, line);
    let text = rope.byte_slice(start..end).to_string();
    text[indent.len().min(text.len())..].to_string()
}

fn is_ordered_at_indent(rope: &Rope, line: usize, indent: &str) -> bool {
    let line_indent = caret_line_indent_for(rope, line);
    // Deeper indent = nested content owned by the most recent parent
    // ordered item; transparent to the walk (we don't renumber it but
    // we don't stop on it either).
    if line_indent.len() > indent.len() && line_indent.starts_with(indent) {
        return true;
    }
    if line_indent != indent {
        return false;
    }
    let body = line_body(rope, line, indent);
    detect_list_marker(&body)
        .map(|m| matches!(m.kind, ListMarkerKind::Ordered(_)))
        .unwrap_or(false)
}

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
