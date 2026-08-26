//! Block-scope guarding for Markdown block-marker toggles.
//!
//! Every block toggle (bullet, numbered, task, checkbox, blockquote,
//! heading) is a line-prefix rewrite: it replaces the covered lines and
//! never looks at their neighbours. That is unsafe in CommonMark, because
//! a plain line following a marked line is folded into the marked block as
//! *lazy continuation* text. Prefixing `- ` to
//!
//! ```text
//! alpha
//! beta
//! ```
//!
//! therefore pulls the untouched `beta` inside the new list item, and the
//! reverse toggle has the mirror defect: stripping the marker from the
//! middle item of a list drops that line into the *previous* item.
//!
//! [`finalize_block_toggle_specs`] wraps
//! [`crate::edit_planning::finalize_specs`] and inserts a blank-line
//! separator wherever the rewrite would otherwise pull an untouched
//! neighbour into (or newly join it to) the toggled run. The separator is
//! part of the same [`SelectionEditPlan`], so it lands in the toggle's
//! single undo group.
//!
//! Chosen resolution: **split the paragraph**, never indent the following
//! lines under the new item. Indenting makes the absorption explicit
//! rather than preventing it, which is the behaviour writers report as
//! wrong. Separators are only ever *inserted*, never auto-removed: a blank
//! line the writer typed is indistinguishable from one a toggle inserted,
//! and silently deleting it would merge paragraphs the writer separated.

use std::collections::HashMap;

use continuity_text::{Position, Selection};
use ropey::Rope;

use crate::edit_markdown::{line_text, split_leading_list_marker};
use crate::edit_planning::{finalize_specs, line_content_end, EditSpec};
use crate::selection_edit::SelectionEditPlan;
use crate::Error;

/// Container block a line opens, when it opens one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContainerKind {
    /// A list item (`- `, `* `, `+ `, `N. `, `N) `).
    ListItem,
    /// A blockquote (`> `).
    BlockQuote,
}

/// Finalize a block-marker toggle's specs, inserting blank-line separators
/// where the rewrite would otherwise change an untouched neighbour's block.
///
/// `specs` must be whole-line content replacements (the shape every block
/// toggle planner produces); any spec that is not recognised as one is
/// simply left out of the neighbour analysis.
///
/// # Errors
///
/// Returns [`Error`] when a separator's byte offset cannot be resolved to a
/// rope position.
pub(crate) fn finalize_block_toggle_specs(
    rope: &Rope,
    mut specs: Vec<EditSpec>,
    selections_before: Vec<Selection>,
    selections_after: Vec<Selection>,
) -> Result<Option<SelectionEditPlan>, Error> {
    if specs.is_empty() {
        return Ok(None);
    }
    let rewritten = collect_rewritten_lines(rope, &specs);
    let separator_lines = compute_separator_lines(rope, &rewritten);
    if separator_lines.is_empty() {
        return Ok(finalize_specs(specs, selections_before, selections_after));
    }
    for &line in &separator_lines {
        let at = line_content_end(rope, line);
        specs.push(EditSpec::insert(rope, at, line_ending_at(rope, line))?);
    }
    let selections_after = shift_selections_past_separators(&selections_after, &separator_lines);
    Ok(finalize_specs(specs, selections_before, selections_after))
}

/// The line ending `line` already carries, so a separator inserted after it
/// matches the document instead of minting a bare `\n` in a CRLF buffer.
/// Falls back to `\n` on the last line, which has no terminator to copy.
fn line_ending_at(rope: &Rope, line: usize) -> String {
    let content_end = line_content_end(rope, line);
    let line_end = if line + 1 < rope.len_lines() {
        rope.line_to_byte(line + 1)
    } else {
        rope.len_bytes()
    };
    let ending = rope.byte_slice(content_end..line_end).to_string();
    if ending.is_empty() {
        "\n".to_string()
    } else {
        ending
    }
}

/// Map every whole-line-replacement spec to `(line, post-toggle text)`,
/// sorted ascending by line.
fn collect_rewritten_lines(rope: &Rope, specs: &[EditSpec]) -> Vec<(usize, String)> {
    let mut out = Vec::with_capacity(specs.len());
    for spec in specs {
        if spec.inserted.contains('\n') {
            continue;
        }
        let line = rope.byte_to_line(spec.start);
        if rope.line_to_byte(line) != spec.start || line_content_end(rope, line) != spec.end {
            continue;
        }
        out.push((line, spec.inserted.clone()));
    }
    out.sort_by_key(|(line, _)| *line);
    out.dedup_by_key(|(line, _)| *line);
    out
}

/// Lines after whose content a `\n` must be inserted so the toggle stays
/// scoped to the lines it rewrote.
fn compute_separator_lines(rope: &Rope, rewritten: &[(usize, String)]) -> Vec<usize> {
    if rewritten.is_empty() {
        return Vec::new();
    }
    let new_text: HashMap<usize, &str> = rewritten
        .iter()
        .map(|(line, text)| (*line, text.as_str()))
        .collect();
    let mut out = Vec::new();
    for (first, last) in contiguous_runs(rewritten) {
        if let Some(line) = leading_separator(rope, &new_text, first) {
            out.push(line);
        }
        if let Some(line) = trailing_separator(rope, &new_text, last) {
            out.push(line);
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Separator needed *before* the run: the untouched line above would newly
/// swallow the run's first line (the marker that used to interrupt its
/// paragraph is gone).
fn leading_separator(rope: &Rope, new_text: &HashMap<usize, &str>, first: usize) -> Option<usize> {
    if first == 0 || new_text.contains_key(&(first - 1)) {
        return None;
    }
    let previous = line_text(rope, first - 1);
    if previous.is_empty() {
        return None;
    }
    let old = line_text(rope, first);
    let new = new_text.get(&first)?;
    let was_joined = is_joined(&previous, &old);
    let is_now_joined = is_joined(&previous, new);
    (is_now_joined && !was_joined).then_some(first - 1)
}

/// Separator needed *after* the run: the untouched line below would either
/// be newly joined to the run's last line, or be pulled inside a container
/// block the toggle just opened on it.
fn trailing_separator(rope: &Rope, new_text: &HashMap<usize, &str>, last: usize) -> Option<usize> {
    let next = last + 1;
    if next >= rope.len_lines() || new_text.contains_key(&next) {
        return None;
    }
    let following = line_text(rope, next);
    let old = line_text(rope, last);
    if old.is_empty() {
        // An empty source line shares its byte offset with the separator
        // insert point; no reachable toggle needs a guard here, and
        // skipping keeps the two inserts from racing for the same offset.
        return None;
    }
    let new = new_text.get(&last)?;
    if !is_joined(new, &following) {
        return None;
    }
    let was_joined = is_joined(&old, &following);
    let opened_container = match container_kind(new) {
        Some(kind) => Some(kind) != container_kind(&old),
        None => false,
    };
    (!was_joined || opened_container).then_some(last)
}

/// Maximal runs of consecutive rewritten lines, as `(first, last)` pairs.
fn contiguous_runs(rewritten: &[(usize, String)]) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut start = rewritten[0].0;
    let mut previous = start;
    for &(line, _) in &rewritten[1..] {
        if line != previous + 1 {
            runs.push((start, previous));
            start = line;
        }
        previous = line;
    }
    runs.push((start, previous));
    runs
}

/// `true` when CommonMark folds `following` into `leading`'s block as lazy
/// continuation text.
fn is_joined(leading: &str, following: &str) -> bool {
    is_paragraph_continuable_line(leading) && is_lazy_continuation_line(following)
}

/// `true` when the line's block can absorb a following lazy-continuation
/// line — that is, the line ends in paragraph content, including a
/// paragraph nested inside a list item or blockquote.
fn is_paragraph_continuable_line(text: &str) -> bool {
    let body = text.trim_start_matches([' ', '\t']);
    if body.trim().is_empty() || starts_leaf_block(body) {
        return false;
    }
    !strip_container_markers(body).trim().is_empty()
}

/// `true` when the line is folded into the preceding paragraph rather than
/// starting a block of its own.
fn is_lazy_continuation_line(text: &str) -> bool {
    let indent: usize = text
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum();
    let body = text.trim_start_matches([' ', '\t']);
    if body.trim().is_empty() {
        return false;
    }
    // Four-or-more-space indentation would be an indented code block, and
    // indented code cannot interrupt a paragraph — the line stays
    // continuation text whatever it looks like.
    if indent >= 4 {
        return true;
    }
    !starts_new_block(body)
}

/// `true` when `body` (already stripped of leading whitespace) opens a
/// block that interrupts an open paragraph.
fn starts_new_block(body: &str) -> bool {
    if starts_leaf_block(body) || body.starts_with('>') {
        return true;
    }
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = body.strip_prefix(marker) {
            // An empty list item cannot interrupt a paragraph.
            return !rest.trim().is_empty();
        }
    }
    // Only a list numbered `1` may interrupt a paragraph.
    if let Some(rest) = body
        .strip_prefix("1. ")
        .or_else(|| body.strip_prefix("1) "))
    {
        return !rest.trim().is_empty();
    }
    false
}

/// `true` for leaf blocks that neither continue nor are continued: ATX
/// headings, fences, thematic breaks, and HTML blocks.
fn starts_leaf_block(body: &str) -> bool {
    is_atx_heading(body)
        || body.starts_with("```")
        || body.starts_with("~~~")
        || is_thematic_break(body)
        || body.starts_with('<')
}

fn is_atx_heading(body: &str) -> bool {
    let hashes = body.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return false;
    }
    matches!(body[hashes..].chars().next(), None | Some(' ') | Some('\t'))
}

fn is_thematic_break(body: &str) -> bool {
    let stripped: String = body.chars().filter(|c| *c != ' ' && *c != '\t').collect();
    stripped.len() >= 3
        && ['-', '*', '_']
            .iter()
            .any(|marker| stripped.chars().all(|c| c == *marker))
}

/// Peel leading blockquote and list markers off a line body.
fn strip_container_markers(body: &str) -> &str {
    let mut rest = body;
    loop {
        if let Some(stripped) = rest.strip_prefix('>') {
            rest = stripped.trim_start_matches([' ', '\t']);
            continue;
        }
        let (marker, after) = split_leading_list_marker(rest);
        if marker.is_empty() {
            return rest;
        }
        rest = after;
    }
}

/// The container block `text` opens, when it opens one.
fn container_kind(text: &str) -> Option<ContainerKind> {
    let body = text.trim_start_matches([' ', '\t']);
    if body.starts_with('>') {
        return Some(ContainerKind::BlockQuote);
    }
    let (marker, _) = split_leading_list_marker(body);
    (!marker.is_empty()).then_some(ContainerKind::ListItem)
}

/// Push every selection endpoint below an inserted separator down one line.
fn shift_selections_past_separators(
    selections: &[Selection],
    separator_lines: &[usize],
) -> Vec<Selection> {
    let shift = |position: Position| -> Position {
        let moved = separator_lines
            .iter()
            .filter(|&&line| (position.line as usize) > line)
            .count();
        Position::new(
            position.line.saturating_add(moved as u32),
            position.byte_in_line,
        )
    };
    selections
        .iter()
        .map(|selection| Selection {
            anchor: shift(selection.anchor),
            head: shift(selection.head),
            kind: selection.kind,
        })
        .collect()
}

#[cfg(test)]
mod tests;
