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
use continuity_text::{Position, Selection};

use crate::edit_planning::{finalize_specs, line_content_end, EditSpec};
use crate::selection_edit::SelectionEditPlan;
use crate::Error;

/// Collapse every line break within the lines covered by the selection into a
/// single space, joining the whole covered block into one line (VS Code
/// "Join Lines" semantics).
///
/// Distinct from [`plan_join_lines`], which only folds the single line below
/// each caret (Vim `J`). The covered lines are derived via
/// [`crate::edit_lines::lines_covered`], so multiple or non-contiguous
/// selections merge into one contiguous `first..=last` block and produce a
/// single [`EditSpec::replace`] — one undo group.
///
/// Whitespace policy mirrors [`plan_join_lines`]: the first covered line is
/// kept verbatim (its leading indentation is preserved); every subsequent
/// line is `trim_start`ed and stripped of a trailing `\r`, then joined with a
/// single space when it has remaining content (blank lines contribute
/// nothing). A single trailing newline on the block is preserved so the line
/// after the selection is not pulled into the join. Returns `Ok(None)` for a
/// single covered line or when the rebuilt block equals the original (no-op,
/// no undo group). The caret lands at the end of the joined content on line
/// `first`, honouring "layout shifts preserve caret-line screen y".
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
    for fragment in covered {
        let trimmed = fragment.trim_start().trim_end_matches('\r');
        if !trimmed.is_empty() {
            joined.push(' ');
            joined.push_str(trimmed);
        }
    }
    if trailing_newline {
        joined.push('\n');
    }
    if joined == block_text {
        return Ok(None);
    }

    let joined_first_line = match joined.find('\n') {
        Some(newline_byte) => &joined[..newline_byte],
        None => &joined[..],
    };
    let caret = Position::new(first as u32, joined_first_line.len() as u32);
    let selections_after = vec![Selection::caret_at(caret)];

    let specs = vec![EditSpec::replace(rope, block_start, block_end, joined)?];
    Ok(finalize_specs(specs, selections_before, selections_after))
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
