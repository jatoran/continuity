//! Line-block movement: `plan_move_line_up` / `plan_move_line_down` and
//! their shared `move_line_block` worker.
//!
//! Extracted from [`crate::edit_lines`] in Phase 17.9 §J cleanup to keep
//! that file under the 600-line cap. The split is by responsibility:
//! line-block *movement* is a self-contained sub-feature distinct from
//! the rest of the line-structure edits (insert / duplicate / join /
//! delete-to-line-edge). Re-exported through `edit_lines` so existing
//! callers (`selection_edit.rs`) keep their import paths.

use continuity_buffer::Buffer;
use continuity_text::{Position, Selection, SelectionKind};

use crate::edit_planning::{finalize_specs, line_content_end, EditSpec};
use crate::selection_edit::SelectionEditPlan;
use crate::Error;

/// Join the lines covered by the selection, one structural level per press
/// (Ctrl+Shift+J semantics).
///
/// Distinct from [`plan_join_lines`], which only folds the single line below
/// each caret (Vim `J`). The covered lines are derived via
/// [`crate::edit_lines::lines_covered`], so multiple or non-contiguous
/// selections merge into one contiguous `first..=last` block and produce a
/// single [`EditSpec::replace`] — one undo group.
///
/// Per-press policy:
/// - **Adjacent content lines** join with a single space. The continuation
///   line is `trim_start`ed, stripped of a trailing `\r`, and stripped of a
///   leading markdown list marker (`- ` / `* ` / `+ ` / `N. ` / `N) `, plus
///   an optional task checkbox) so joining bullet items concatenates their
///   content instead of embedding literal markers mid-line. A dash without
///   a following space is ordinary text and survives.
/// - **Blank-line separators** lose exactly one newline: `a\n\nb` becomes
///   `a\nb` — the sections move together but stay on separate lines.
///   Pressing again joins them with a space, so spamming the chord
///   converges on one line while preserving deliberate section structure
///   on the way.
///
/// The replacement selection covers the whole rebuilt block, which is what
/// makes the spamming workflow possible. A single trailing newline on the
/// block is preserved so the line after the selection is never pulled in.
/// Returns `Ok(None)` for a single covered line or when the rebuilt block
/// equals the original (no-op, no undo group).
pub(crate) fn plan_join_selected_lines(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let lines = crate::edit_lines::lines_covered(buffer);
    if lines.is_empty() {
        return Ok(None);
    }
    let first = *lines.first().expect("lines is non-empty by check");
    let last = *lines.last().expect("lines is non-empty by check");
    if first == last {
        // A single covered line has no interior break to remove. Keep this
        // distinct from the Vim-J `plan_join_lines` one-below behaviour.
        return Ok(None);
    }
    let block_start = rope.line_to_byte(first);
    let block_end = if last + 1 < rope.len_lines() {
        rope.line_to_byte(last + 1)
    } else {
        rope.len_bytes()
    };
    let block_text = rope.byte_slice(block_start..block_end).to_string();
    let trailing_newline = block_text.ends_with('\n');
    let body = if trailing_newline {
        &block_text[..block_text.len() - 1]
    } else {
        &block_text[..]
    };

    let mut covered = body.split('\n');
    // Keep the first covered line verbatim so its leading indentation
    // survives; `body` is non-empty so at least one segment exists.
    let mut joined = covered.next().unwrap_or("").to_string();
    let mut pending_blank_lines = 0usize;
    for fragment in covered {
        let raw = fragment.trim_end_matches('\r');
        let trimmed = raw.trim_start();
        if trimmed.is_empty() {
            pending_blank_lines += 1;
            continue;
        }
        if pending_blank_lines > 0 {
            // Section break: drop exactly one blank line, keep the rest,
            // and keep this section on its own line (indentation intact).
            for _ in 1..pending_blank_lines {
                joined.push('\n');
            }
            joined.push('\n');
            joined.push_str(raw);
            pending_blank_lines = 0;
            continue;
        }
        let content = strip_joined_line_marker(trimmed);
        if !content.is_empty() {
            joined.push(' ');
            joined.push_str(content);
        }
    }
    // Blank lines at the end of the block also lose exactly one newline.
    for _ in 1..pending_blank_lines.max(1) {
        joined.push('\n');
    }
    if trailing_newline {
        joined.push('\n');
    }
    if joined == block_text {
        return Ok(None);
    }

    // Keep the whole rebuilt block selected so the chord can be pressed
    // repeatedly until everything sits on one line.
    let joined_body = if trailing_newline {
        &joined[..joined.len() - 1]
    } else {
        &joined[..]
    };
    let extra_lines = joined_body.matches('\n').count();
    let last_line_text = joined_body.rsplit('\n').next().unwrap_or("");
    let selections_after = vec![Selection::new(
        Position::new(first as u32, 0),
        Position::new((first + extra_lines) as u32, last_line_text.len() as u32),
        SelectionKind::Caret,
    )];

    let specs = vec![EditSpec::replace(rope, block_start, block_end, joined)?];
    Ok(finalize_specs(specs, selections_before, selections_after))
}

/// Strip a leading markdown list marker (`- `, `* `, `+ `, `N. `, `N) `)
/// and an optional task checkbox (`[ ] ` / `[x] `) from a join
/// continuation line. Text whose dash is not a list marker (no following
/// space) is returned unchanged.
fn strip_joined_line_marker(text: &str) -> &str {
    let rest = if let Some(rest) = text
        .strip_prefix("- ")
        .or_else(|| text.strip_prefix("* "))
        .or_else(|| text.strip_prefix("+ "))
    {
        rest
    } else {
        let bytes = text.as_bytes();
        let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
        if digits > 0
            && digits + 1 < bytes.len()
            && (bytes[digits] == b'.' || bytes[digits] == b')')
            && bytes[digits + 1] == b' '
        {
            &text[digits + 2..]
        } else {
            return text;
        }
    };
    for checkbox in ["[ ] ", "[x] ", "[X] "] {
        if let Some(after) = rest.strip_prefix(checkbox) {
            return after;
        }
    }
    rest
}

pub(crate) fn plan_join_lines(buffer: &Buffer) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let mut specs = Vec::new();
    let mut handled = std::collections::HashSet::new();
    let mut selections_after = Vec::new();
    for selection in &selections_before {
        let line = selection.head.line as usize;
        if !handled.insert(line) {
            selections_after.push(*selection);
            continue;
        }
        if line + 1 >= rope.len_lines() {
            selections_after.push(*selection);
            continue;
        }
        let line_end = line_content_end(rope, line);
        let next_start = rope.line_to_byte(line + 1);
        let next_text = rope
            .byte_slice(next_start..line_content_end(rope, line + 1))
            .to_string();
        let trimmed = next_text.trim_start();
        let separator = if trimmed.is_empty() { "" } else { " " };
        let replacement = format!("{separator}{trimmed}");
        let consume_end = next_start + (next_text.len() - trimmed.len()) + trimmed.len();
        specs.push(EditSpec::replace(rope, line_end, consume_end, replacement)?);
        selections_after.push(*selection);
    }
    Ok(finalize_specs(specs, selections_before, selections_after))
}

pub(crate) fn plan_move_line_up(buffer: &Buffer) -> Result<Option<SelectionEditPlan>, Error> {
    move_line_block(buffer, -1)
}

pub(crate) fn plan_move_line_down(buffer: &Buffer) -> Result<Option<SelectionEditPlan>, Error> {
    move_line_block(buffer, 1)
}

fn move_line_block(buffer: &Buffer, delta: i32) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let lines = crate::edit_lines::lines_covered(buffer);
    if lines.is_empty() {
        return Ok(None);
    }
    let first = *lines.first().expect("lines is non-empty by check");
    let last = *lines.last().expect("lines is non-empty by check");
    if delta < 0 && first == 0 {
        return Ok(None);
    }
    if delta > 0 && last + 1 >= rope.len_lines() {
        return Ok(None);
    }

    // Auto-renumber: when the moved block and the line it swaps with all
    // live inside the same contiguous ordered-list run, reorder + renumber
    // that run as a single replacement so the markers stay `1.`, `2.`, …
    // after the move (one undo group). Falls through to the generic block
    // move otherwise.
    if let Some(plan) = try_move_within_ordered_run(buffer, first, last, delta)? {
        return Ok(Some(plan));
    }
    let block_start = rope.line_to_byte(first);
    let block_end = if last + 1 < rope.len_lines() {
        rope.line_to_byte(last + 1)
    } else {
        rope.len_bytes()
    };
    let block_text = rope.byte_slice(block_start..block_end).to_string();

    let (replace_start, replace_end, combined_raw) = if delta < 0 {
        let prev_start = rope.line_to_byte(first - 1);
        let prev_text = rope.byte_slice(prev_start..block_start).to_string();
        let mut block = block_text.clone();
        if !block.ends_with('\n') {
            block.push('\n');
        }
        (prev_start, block_end, format!("{block}{prev_text}"))
    } else {
        let next_end = if last + 2 < rope.len_lines() {
            rope.line_to_byte(last + 2)
        } else {
            rope.len_bytes()
        };
        let next_text = rope.byte_slice(block_end..next_end).to_string();
        let mut next_pad = next_text.clone();
        if !next_pad.ends_with('\n') {
            next_pad.push('\n');
        }
        (block_start, next_end, format!("{next_pad}{block_text}"))
    };
    let original_len = replace_end - replace_start;
    let mut replacement = combined_raw;
    while replacement.len() > original_len && replacement.ends_with('\n') {
        replacement.pop();
    }
    let specs = vec![EditSpec::replace(
        rope,
        replace_start,
        replace_end,
        replacement,
    )?];
    // Translate both endpoints so the selection range (and kind) carries
    // through the move — otherwise a multi-line highlight collapses to a
    // caret on each press and the user can't keep shoving it up/down.
    let shift = |p: Position| -> Position {
        let new_line = if delta < 0 {
            p.line.saturating_sub(1)
        } else {
            p.line.saturating_add(1)
        };
        Position::new(new_line, p.byte_in_line)
    };
    let mut selections_after = Vec::new();
    for selection in &selections_before {
        selections_after.push(Selection::new(
            shift(selection.anchor),
            shift(selection.head),
            selection.kind,
        ));
    }
    Ok(finalize_specs(specs, selections_before, selections_after))
}

/// Reorder + renumber an ordered-list run when the moved block
/// (`first..=last`) and the line it swaps with (`first - 1` for an up move,
/// `last + 1` for a down move) all sit inside the same contiguous ordered
/// run at the same indent. Rewrites the whole run as one replacement so the
/// `N.` markers stay sequential. Returns `Ok(None)` when the move is not a
/// pure within-run reorder, so the caller falls back to the generic block
/// move.
fn try_move_within_ordered_run(
    buffer: &Buffer,
    first: usize,
    last: usize,
    delta: i32,
) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    // The swap partner that the block trades places with.
    let partner = if delta < 0 { first - 1 } else { last + 1 };

    // Anchor the run on the caret block's first line. Every line in the
    // block, plus the partner, must be ordered items at the run's indent.
    let Some((run_start, run_end, indent)) = crate::edit_list::ordered_list_range_for(rope, first)
    else {
        return Ok(None);
    };
    if first < run_start || last > run_end || partner < run_start || partner > run_end {
        return Ok(None);
    }
    // Require every covered + partner line to be an ordered item — a nested
    // or non-ordered line inside the span means the simple positional
    // renumber would be wrong, so defer to the generic move.
    for line in first..=last {
        if !is_ordered_line_at(rope, line, &indent) {
            return Ok(None);
        }
    }
    if !is_ordered_line_at(rope, partner, &indent) {
        return Ok(None);
    }

    // Pull the run's bodies (text after the `N. ` marker) in source order,
    // then move the block's bodies past the partner.
    let mut bodies: Vec<String> = Vec::with_capacity(run_end - run_start + 1);
    for line in run_start..=run_end {
        bodies.push(ordered_body_at(rope, line, &indent));
    }
    let block_lo = first - run_start;
    let block_hi = last - run_start;
    let block: Vec<String> = bodies[block_lo..=block_hi].to_vec();
    // Remove the block, then reinsert it shifted by one position.
    bodies.drain(block_lo..=block_hi);
    let insert_at = if delta < 0 {
        block_lo - 1
    } else {
        block_lo + 1
    };
    for (offset, body) in block.into_iter().enumerate() {
        bodies.insert(insert_at + offset, body);
    }

    // Rebuild the run with sequential markers.
    let mut rebuilt = String::new();
    for (idx, body) in bodies.iter().enumerate() {
        if idx > 0 {
            rebuilt.push('\n');
        }
        rebuilt.push_str(&format!("{indent}{}. {body}", idx + 1));
    }

    let block_start = rope.line_to_byte(run_start);
    let block_end = if run_end + 1 < rope.len_lines() {
        rope.line_to_byte(run_end + 1)
    } else {
        rope.len_bytes()
    };
    let original = rope.byte_slice(block_start..block_end).to_string();
    let trailing_newline = original.ends_with('\n');
    if trailing_newline {
        rebuilt.push('\n');
    }
    if rebuilt == original {
        return Ok(None);
    }

    let selections_before = buffer.selections().to_vec();
    let shift = |p: Position| -> Position {
        let new_line = if delta < 0 {
            p.line.saturating_sub(1)
        } else {
            p.line.saturating_add(1)
        };
        Position::new(new_line, p.byte_in_line)
    };
    let selections_after: Vec<Selection> = selections_before
        .iter()
        .map(|sel| Selection::new(shift(sel.anchor), shift(sel.head), sel.kind))
        .collect();

    let specs = vec![EditSpec::replace(rope, block_start, block_end, rebuilt)?];
    Ok(finalize_specs(specs, selections_before, selections_after))
}

/// Body text of an ordered-list line (everything after the `N. ` marker),
/// or the whole post-indent body when no ordered marker is present.
fn ordered_body_at(rope: &ropey::Rope, line: usize, indent: &str) -> String {
    let start = rope.line_to_byte(line);
    let end = line_content_end(rope, line);
    let text = rope.byte_slice(start..end).to_string();
    if text.len() < indent.len() {
        return text;
    }
    let body = &text[indent.len()..];
    match crate::edit_list::detect_list_marker(body) {
        Some(marker) => body[marker.prefix_len..].to_string(),
        None => body.to_string(),
    }
}

/// Whether `line` is an ordered-list item at exactly `indent`.
fn is_ordered_line_at(rope: &ropey::Rope, line: usize, indent: &str) -> bool {
    let start = rope.line_to_byte(line);
    let end = line_content_end(rope, line);
    let text = rope.byte_slice(start..end).to_string();
    if text.len() < indent.len() || !text.starts_with(indent) {
        return false;
    }
    let line_indent: String = text
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    if line_indent != indent {
        return false;
    }
    let body = &text[indent.len()..];
    crate::edit_list::detect_list_marker(body)
        .is_some_and(|m| crate::edit_list::is_ordered_marker(&m))
}

#[cfg(test)]
mod tests {
    use continuity_buffer::Buffer;
    use continuity_text::{Position, Selection};

    use crate::selection_edit::{apply_plan, plan, SelectionEdit};

    fn build(text: &str, line: u32, col: u32) -> Buffer {
        let mut b = Buffer::from_text(text);
        b.set_selections(vec![Selection::caret_at(Position::new(line, col))]);
        b
    }

    fn run(b: &mut Buffer, edit: SelectionEdit) {
        let p = plan(b, &edit).expect("plan ok").expect("plan some");
        apply_plan(b, &p).expect("apply ok");
    }

    #[test]
    fn move_up_within_ordered_run_renumbers() {
        // Move `2. b` up past `1. a`; markers stay sequential (1,2,3).
        let mut b = build("1. a\n2. b\n3. c", 1, 0);
        run(&mut b, SelectionEdit::MoveLineUp);
        assert_eq!(b.rope().to_string(), "1. b\n2. a\n3. c");
    }

    #[test]
    fn move_down_within_ordered_run_renumbers() {
        let mut b = build("1. a\n2. b\n3. c", 1, 0);
        run(&mut b, SelectionEdit::MoveLineDown);
        assert_eq!(b.rope().to_string(), "1. a\n2. c\n3. b");
    }

    #[test]
    fn move_first_ordered_item_down_renumbers() {
        let mut b = build("1. a\n2. b\n3. c", 0, 0);
        run(&mut b, SelectionEdit::MoveLineDown);
        assert_eq!(b.rope().to_string(), "1. b\n2. a\n3. c");
    }

    #[test]
    fn move_non_ordered_block_falls_through_to_plain_move() {
        // Plain text move is unaffected by the renumber path.
        let mut b = build("a\nb\nc", 1, 0);
        run(&mut b, SelectionEdit::MoveLineUp);
        assert_eq!(b.rope().to_string(), "b\na\nc");
    }

    #[test]
    fn move_ordered_item_out_to_plain_line_does_not_corrupt() {
        // Moving the top ordered item up past a plain line is not a
        // within-run reorder, so it falls through to the generic move
        // (no renumber); the markers are simply repositioned verbatim.
        let mut b = build("intro\n1. a\n2. b", 1, 0);
        run(&mut b, SelectionEdit::MoveLineUp);
        assert_eq!(b.rope().to_string(), "1. a\nintro\n2. b");
    }
}
