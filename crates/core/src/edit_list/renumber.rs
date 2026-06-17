//! Ordered-list renumbering: the explicit `markdown.renumber_list` command,
//! the shared run-rewrite helper, the ordered-run locator, and the
//! smart-newline ordered-continue path that renumbers in the same undo
//! group.
//!
//! Split out of `edit_list.rs` so that file stays under the 600-line cap.
//! Everything here works on ordered (`N.`) markers at a shared indent;
//! detection reuses [`super::detect_list_marker`].

use continuity_buffer::Buffer;
use continuity_text::{Position, Selection};
use ropey::Rope;

use crate::edit_planning::{finalize_specs, line_content_end, EditSpec};
use crate::selection_edit::SelectionEditPlan;
use crate::Error;

use super::{
    detect_list_marker, list_newline_action, ListMarker, ListMarkerKind, ListNewlineAction,
};

/// Continue an ordered list at `selection`'s caret and renumber the whole
/// contiguous ordered run in the same plan. Returns `Ok(None)` when the
/// caret line is not an ordered-list continuation (the caller falls back to
/// the generic per-selection path) — including empty-stub lines, which must
/// *end* the list rather than continue it.
pub(crate) fn try_ordered_continue_with_renumber(
    rope: &Rope,
    selection: Selection,
) -> Result<Option<SelectionEditPlan>, Error> {
    let head = selection.head;
    let line = head.line as usize;
    let line_start = rope.line_to_byte(line);
    let line_end = line_content_end(rope, line);
    let line_text = rope.byte_slice(line_start..line_end).to_string();
    let indent = crate::edit_lines::leading_whitespace_for(rope, line);
    if line_text.len() < indent.len() {
        return Ok(None);
    }
    let body = &line_text[indent.len()..];

    // Only the ordered-Continue action renumbers. Task lines, unordered
    // lists, empty stubs, and non-list lines fall through.
    let is_ordered =
        detect_list_marker(body).is_some_and(|m| matches!(m.kind, ListMarkerKind::Ordered(_)));
    if !is_ordered {
        return Ok(None);
    }
    // A `- [ ] ` style task that happens to be ordered keeps the task
    // continuation logic; an empty ordered item ends the list. Defer to
    // `list_newline_action` for the exact decision and only proceed when
    // it asks to Continue with a bare ordered marker.
    match list_newline_action(&indent, body, "  ") {
        ListNewlineAction::Continue { ref next_marker }
            if next_marker.ends_with(". ") && !next_marker.contains('[') =>
        {
            // Ordinary ordered continuation — fall through to renumber.
        }
        _ => return Ok(None),
    }

    let Some((start_line, end_line, run_indent)) = ordered_list_range(rope, line) else {
        return Ok(None);
    };

    let byte = head.to_byte_offset(rope)?;
    // The inserted line's final number is its 1-based position within the
    // renumbered run: lines `start_line..=line` occupy 1..=(line-start+1),
    // so the new item is `(line - start_line) + 2`.
    let new_number = (line - start_line + 2) as u32;
    let inserted = format!("\n{run_indent}{new_number}. ");

    let mut specs = vec![EditSpec::insert(rope, byte, inserted.clone())?];
    // Renumber the existing run lines to 1..=(run length). The inserted
    // line already carries `new_number`; renumbering the old lines makes
    // the lines *after* the caret shift up by one and corrects any prior
    // mis-numbering. We renumber against the pre-edit rope and let
    // `finalize_specs` order everything descending — the insert at the
    // line-content boundary never overlaps a line-content replace because
    // a replace ends at `line_content_end` while the next line's replace
    // starts at the following `line_to_byte`, and the insert carries a
    // non-empty payload so `merge_specs` never folds it into a delete.
    for renumber_line in start_line..=end_line {
        let rl_start = rope.line_to_byte(renumber_line);
        let rl_end = line_content_end(rope, renumber_line);
        let rl_text = rope.byte_slice(rl_start..rl_end).to_string();
        if rl_text.len() < run_indent.len() || !rl_text.starts_with(&run_indent) {
            continue;
        }
        let rl_body = &rl_text[run_indent.len()..];
        let Some(rl_marker) = detect_list_marker(rl_body) else {
            continue;
        };
        if !matches!(rl_marker.kind, ListMarkerKind::Ordered(_)) {
            continue;
        }
        // Final number: positions up to and including the caret line keep
        // their 1-based index; lines after the caret line shift up by one
        // to make room for the inserted item.
        let position_in_run = renumber_line - start_line + 1;
        let final_number = if renumber_line <= line {
            position_in_run as u32
        } else {
            (position_in_run + 1) as u32
        };
        let after_marker = &rl_body[rl_marker.prefix_len..];
        let new_line = format!("{run_indent}{final_number}. {after_marker}");
        if new_line != rl_text {
            specs.push(EditSpec::replace(rope, rl_start, rl_end, new_line)?);
        }
    }

    // Land the caret at the end of the inserted marker on the new line.
    // Computed from the inserted text rather than `advance_position` on the
    // (possibly renumbered) caret line so a width change in the caret
    // line's own marker can't shift the post-edit column.
    let new_line_index = head.line.saturating_add(1);
    let new_col = (run_indent.len() + format!("{new_number}. ").len()) as u32;
    let selections_after = vec![Selection::caret_at(Position::new(new_line_index, new_col))];
    Ok(finalize_specs(specs, vec![selection], selections_after))
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
        specs.extend(renumber_run_specs(rope, start_line, end_line, &indent)?);
    }
    Ok(finalize_specs(
        specs,
        selections_before.clone(),
        selections_before,
    ))
}

/// Rewrite the ordered markers of every line in `start_line..=end_line`
/// (at shared `indent`) as `1.`, `2.`, `3.`, … against `rope`, emitting one
/// [`EditSpec`] per line whose marker actually changes. Shared by the
/// explicit renumber command, the smart-newline ordered-continue path, and
/// the move-line reorder path so all three keep one numbering policy.
pub(crate) fn renumber_run_specs(
    rope: &Rope,
    start_line: usize,
    end_line: usize,
    indent: &str,
) -> Result<Vec<EditSpec>, Error> {
    let mut specs = Vec::new();
    let mut counter: u32 = 1;
    for line in start_line..=end_line {
        let line_start = rope.line_to_byte(line);
        let line_end = line_content_end(rope, line);
        let line_text = rope.byte_slice(line_start..line_end).to_string();
        // Lines deeper than `indent` are nested content owned by the run
        // but not themselves ordered items at this level — leave them be.
        if line_text.len() < indent.len() || !line_text.starts_with(indent) {
            continue;
        }
        let body = &line_text[indent.len()..];
        let Some(marker) = detect_list_marker(body) else {
            continue;
        };
        if !matches!(marker.kind, ListMarkerKind::Ordered(_)) {
            continue;
        }
        let after_marker = &body[marker.prefix_len..];
        let new_line = format!("{indent}{counter}. {after_marker}");
        if new_line != line_text {
            specs.push(EditSpec::replace(rope, line_start, line_end, new_line)?);
        }
        counter = counter.saturating_add(1);
    }
    Ok(specs)
}

/// Locate the ordered-list run containing `caret_line`, exposed so other
/// planners (move-line reorder) can renumber the affected run.
pub(crate) fn ordered_list_range_for(
    rope: &Rope,
    caret_line: usize,
) -> Option<(usize, usize, String)> {
    ordered_list_range(rope, caret_line)
}

/// Whether `marker` is an ordered (`N.`) marker rather than an unordered
/// glyph. Exposed so sibling planners can classify a detected marker
/// without matching on the `pub(crate)` [`ListMarkerKind`] variants.
pub(crate) fn is_ordered_marker(marker: &ListMarker) -> bool {
    matches!(marker.kind, ListMarkerKind::Ordered(_))
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
