//! Markdown block-structure edits — heading rewrites and section moves.
//!
//! These planners reshape the document at the heading/section level. Inline
//! markdown ops (emphasis, list/checkbox/blockquote prefixes, code fences,
//! links, image refs) live in `edit_markdown.rs`. Shared helpers
//! (`heading_level`, `enclosing_heading_line`, etc.) are re-exported from
//! that sibling module.

use continuity_buffer::Buffer;
use continuity_text::{Position, Selection};

use crate::edit_markdown::{
    enclosing_heading_line, heading_level, line_text, lines_in, next_heading_level,
    previous_section_start, section_end_line, strip_heading_prefix,
};
use crate::edit_planning::{finalize_specs, line_content_end, EditSpec};
use crate::selection_edit::SelectionEditPlan;
use crate::Error;

pub(crate) fn plan_markdown_set_heading(
    buffer: &Buffer,
    level: u8,
) -> Result<Option<SelectionEditPlan>, Error> {
    if level > 6 {
        return Err(Error::InvalidArgument {
            name: "markdown.set_heading",
            reason: format!("level {level} is out of range 0..=6"),
        });
    }
    rewrite_heading(buffer, |_current| level)
}

pub(crate) fn plan_markdown_cycle_heading(
    buffer: &Buffer,
    delta: i32,
) -> Result<Option<SelectionEditPlan>, Error> {
    rewrite_heading(buffer, |current| {
        let next = i32::from(current).saturating_add(delta).clamp(0, 6);
        next as u8
    })
}

pub(crate) fn plan_markdown_promote_section(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    rewrite_section_heading(buffer, |current| current.saturating_sub(1).max(1))
}

pub(crate) fn plan_markdown_demote_section(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    rewrite_section_heading(buffer, |current| (current + 1).min(6))
}

pub(crate) fn plan_markdown_move_section_up(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    move_section(buffer, true)
}

pub(crate) fn plan_markdown_move_section_down(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    move_section(buffer, false)
}

fn rewrite_heading<F>(buffer: &Buffer, f: F) -> Result<Option<SelectionEditPlan>, Error>
where
    F: Fn(u8) -> u8,
{
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let lines = lines_in(buffer);
    let mut specs = Vec::new();
    for &line in &lines {
        let start = rope.line_to_byte(line);
        let end = line_content_end(rope, line);
        let text = rope.byte_slice(start..end).to_string();
        let current = heading_level(&text);
        let next = f(current);
        let body = strip_heading_prefix(&text);
        let prefix = if next == 0 {
            String::new()
        } else {
            format!("{} ", "#".repeat(next as usize))
        };
        let replacement = format!("{prefix}{body}");
        if replacement != text {
            specs.push(EditSpec::replace(rope, start, end, replacement)?);
        }
    }
    Ok(finalize_specs(
        specs,
        selections_before.clone(),
        selections_before,
    ))
}

fn rewrite_section_heading<F>(buffer: &Buffer, f: F) -> Result<Option<SelectionEditPlan>, Error>
where
    F: Fn(u8) -> u8,
{
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let mut specs = Vec::new();
    let mut visited = std::collections::HashSet::new();
    for selection in &selections_before {
        let line = selection.head.line as usize;
        if let Some(heading_line) = enclosing_heading_line(rope, line) {
            if !visited.insert(heading_line) {
                continue;
            }
            let start = rope.line_to_byte(heading_line);
            let end = line_content_end(rope, heading_line);
            let text = rope.byte_slice(start..end).to_string();
            let current = heading_level(&text);
            let next = f(current);
            let body = strip_heading_prefix(&text);
            let prefix = if next == 0 {
                String::new()
            } else {
                format!("{} ", "#".repeat(next as usize))
            };
            let replacement = format!("{prefix}{body}");
            if replacement != text {
                specs.push(EditSpec::replace(rope, start, end, replacement)?);
            }
        }
    }
    Ok(finalize_specs(
        specs,
        selections_before.clone(),
        selections_before,
    ))
}

fn move_section(buffer: &Buffer, up: bool) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let head_line = selections_before
        .first()
        .map(|s| s.head.line as usize)
        .unwrap_or(0);
    let Some(start_line) = enclosing_heading_line(rope, head_line) else {
        return Ok(None);
    };
    let level = heading_level(&line_text(rope, start_line));
    let end_line = section_end_line(rope, start_line, level);

    let (other_start, other_end, new_caret_line) = if up {
        if start_line == 0 {
            return Ok(None);
        }
        let prev_end = start_line - 1;
        let Some(prev_start) = previous_section_start(rope, prev_end) else {
            return Ok(None);
        };
        (prev_start, prev_end, prev_start)
    } else {
        if end_line + 1 >= rope.len_lines() {
            return Ok(None);
        }
        let next_start = end_line + 1;
        let Some(next_level) = next_heading_level(rope, next_start) else {
            return Ok(None);
        };
        let next_end = section_end_line(rope, next_start, next_level);
        let new_caret_line = start_line + (next_end - next_start + 1);
        (next_start, next_end, new_caret_line)
    };

    let block_start = rope.line_to_byte(start_line);
    let block_end = if end_line + 1 < rope.len_lines() {
        rope.line_to_byte(end_line + 1)
    } else {
        rope.len_bytes()
    };
    let other_start_byte = rope.line_to_byte(other_start);
    let other_end_byte = if other_end + 1 < rope.len_lines() {
        rope.line_to_byte(other_end + 1)
    } else {
        rope.len_bytes()
    };

    let (replace_start, replace_end, replacement) = if up {
        let block = rope.byte_slice(block_start..block_end).to_string();
        let other = rope.byte_slice(other_start_byte..block_start).to_string();
        let mut combined = block;
        if !combined.ends_with('\n') {
            combined.push('\n');
        }
        combined.push_str(&other);
        (other_start_byte, block_end, combined)
    } else {
        let block = rope.byte_slice(block_start..block_end).to_string();
        let other = rope.byte_slice(block_end..other_end_byte).to_string();
        let mut combined = other;
        if !combined.ends_with('\n') {
            combined.push('\n');
        }
        combined.push_str(&block);
        (block_start, other_end_byte, combined)
    };

    let specs = vec![EditSpec::replace(
        rope,
        replace_start,
        replace_end,
        replacement,
    )?];
    let head = Position::new(new_caret_line as u32, 0);
    Ok(finalize_specs(
        specs,
        selections_before,
        vec![Selection::caret_at(head)],
    ))
}

#[cfg(test)]
mod tests {
    use continuity_buffer::Buffer;
    use continuity_text::{Position, Selection};

    use crate::selection_edit::{apply_plan, plan, SelectionEdit};

    fn caret(line: u32, col: u32) -> Selection {
        Selection::caret_at(Position::new(line, col))
    }

    fn run(buffer: &mut Buffer, edit: SelectionEdit) {
        let plan = plan(buffer, &edit).expect("plan ok").expect("plan some");
        apply_plan(buffer, &plan).expect("apply ok");
    }

    #[test]
    fn set_heading_rewrites_prefix() {
        let mut b = Buffer::from_text("hello");
        b.set_selections(vec![caret(0, 0)]);
        run(&mut b, SelectionEdit::MarkdownSetHeading(2));
        assert_eq!(b.rope().to_string(), "## hello");
    }

    #[test]
    fn set_heading_zero_strips() {
        let mut b = Buffer::from_text("### hello");
        b.set_selections(vec![caret(0, 0)]);
        run(&mut b, SelectionEdit::MarkdownSetHeading(0));
        assert_eq!(b.rope().to_string(), "hello");
    }

    #[test]
    fn cycle_heading_increments() {
        let mut b = Buffer::from_text("# h");
        b.set_selections(vec![caret(0, 0)]);
        run(&mut b, SelectionEdit::MarkdownCycleHeading(1));
        assert_eq!(b.rope().to_string(), "## h");
    }

    #[test]
    fn promote_section_decreases_level() {
        let mut b = Buffer::from_text("## title\nbody");
        b.set_selections(vec![caret(1, 0)]);
        run(&mut b, SelectionEdit::MarkdownPromoteSection);
        assert_eq!(b.rope().to_string(), "# title\nbody");
    }

    #[test]
    fn demote_section_increases_level() {
        let mut b = Buffer::from_text("# title\nbody");
        b.set_selections(vec![caret(1, 0)]);
        run(&mut b, SelectionEdit::MarkdownDemoteSection);
        assert_eq!(b.rope().to_string(), "## title\nbody");
    }

    #[test]
    fn move_section_down_swaps_blocks() {
        let mut b = Buffer::from_text("# a\nbody-a\n# b\nbody-b\n");
        b.set_selections(vec![caret(0, 0)]);
        run(&mut b, SelectionEdit::MarkdownMoveSectionDown);
        assert_eq!(b.rope().to_string(), "# b\nbody-b\n# a\nbody-a\n");
    }

    #[test]
    fn move_section_up_swaps_blocks() {
        let mut b = Buffer::from_text("# a\nbody-a\n# b\nbody-b\n");
        b.set_selections(vec![caret(2, 0)]);
        run(&mut b, SelectionEdit::MarkdownMoveSectionUp);
        assert_eq!(b.rope().to_string(), "# b\nbody-b\n# a\nbody-a\n");
    }
}
