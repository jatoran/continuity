//! Inline markdown edits — emphasis toggles, list/checkbox/blockquote
//! prefixes, code-fence wrapping, link/image insertion.
//!
//! These work on the buffer's source text directly without going through
//! tree-sitter — Phase 6 keeps the planning side string-based; structural
//! parsing remains the renderer/decoration crate's concern. Heading and
//! section reshaping lives in `edit_markdown_blocks.rs` and reuses the
//! shared helpers re-exported from this module.

use continuity_buffer::Buffer;
use continuity_text::{Selection, SelectionKind};
use ropey::Rope;

use crate::edit_planning::{advance_position, finalize_specs, line_content_end, EditSpec};
use crate::selection_edit::SelectionEditPlan;
use crate::EmphasisKind;
use crate::Error;

pub(crate) fn plan_markdown_toggle_emphasis(
    buffer: &Buffer,
    kind: EmphasisKind,
) -> Result<Option<SelectionEditPlan>, Error> {
    let (open, close) = emphasis_delim(kind);
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let mut specs = Vec::new();
    let mut selections_after = Vec::new();
    for selection in &selections_before {
        let range = selection.ordered_range();
        let start = range.start.to_byte_offset(rope)?;
        let end = range.end.to_byte_offset(rope)?;
        if start == end {
            // Insert empty pair, caret between markers.
            let inserted = format!("{open}{close}");
            specs.push(EditSpec::insert(rope, start, inserted)?);
            let head = advance_position(range.start, open);
            selections_after.push(Selection::caret_at(head));
            continue;
        }
        let original = rope.byte_slice(start..end).to_string();
        let replaced = if let Some(stripped) = strip_wrap(&original, open, close) {
            stripped.to_string()
        } else if original.contains('\n') {
            // Markdown emphasis can't span paragraph boundaries — if the
            // selection covers multiple lines, wrap each line's
            // non-whitespace content individually so the parser sees
            // paired markers on every line and applies the styling.
            wrap_multiline(&original, open, close)
        } else {
            format!("{open}{original}{close}")
        };
        specs.push(EditSpec::replace(rope, start, end, replaced.clone())?);
        let new_head = advance_position(range.start, &replaced);
        selections_after.push(Selection::new(range.start, new_head, selection.kind));
    }
    Ok(finalize_specs(specs, selections_before, selections_after))
}

/// Wrap each non-empty line's content in `open` … `close`, preserving
/// the leading and trailing whitespace on every line so the rope's
/// indentation reads identically.
fn wrap_multiline(original: &str, open: &str, close: &str) -> String {
    let mut out = String::with_capacity(original.len() + open.len() * 4 + close.len() * 4);
    let mut first = true;
    for line in original.split('\n') {
        if !first {
            out.push('\n');
        }
        first = false;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            out.push_str(line);
            continue;
        }
        let lead = line.len() - line.trim_start().len();
        let trail = line.len() - line.trim_end().len();
        out.push_str(&line[..lead]);
        out.push_str(open);
        out.push_str(&line[lead..line.len() - trail]);
        out.push_str(close);
        out.push_str(&line[line.len() - trail..]);
    }
    out
}

pub(crate) fn plan_markdown_toggle_bullet(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    toggle_line_prefix(buffer, &["- ", "* ", "+ "], "- ")
}

pub(crate) fn plan_markdown_toggle_numbered(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let lines = lines_in(buffer);
    let mut specs = Vec::new();
    let mut counter = 1_u32;
    for &line in &lines {
        let start = rope.line_to_byte(line);
        let end = line_content_end(rope, line);
        let text = rope.byte_slice(start..end).to_string();
        let leading = leading_whitespace_len(&text);
        let body = &text[leading..];
        if let Some(rest) = strip_numbered_prefix(body) {
            let replacement = format!("{}{rest}", &text[..leading]);
            specs.push(EditSpec::replace(rope, start, end, replacement)?);
        } else {
            let prefix = format!("{}{counter}. ", &text[..leading]);
            counter += 1;
            specs.push(EditSpec::replace(
                rope,
                start,
                end,
                format!("{prefix}{body}"),
            )?);
        }
    }
    Ok(finalize_specs(
        specs,
        selections_before.clone(),
        selections_before,
    ))
}

pub(crate) fn plan_markdown_toggle_checkbox(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let lines = lines_in(buffer);
    let mut specs = Vec::new();
    for &line in &lines {
        let start = rope.line_to_byte(line);
        let end = line_content_end(rope, line);
        let text = rope.byte_slice(start..end).to_string();
        let leading = leading_whitespace_len(&text);
        let body = &text[leading..];
        let (replacement, replaced) = if let Some(rest) = body.strip_prefix("[ ] ") {
            (format!("{}[x] {rest}", &text[..leading]), true)
        } else if let Some(rest) = body.strip_prefix("[x] ") {
            (format!("{}[ ] {rest}", &text[..leading]), true)
        } else {
            (format!("{}[ ] {body}", &text[..leading]), true)
        };
        if replaced {
            specs.push(EditSpec::replace(rope, start, end, replacement)?);
        }
    }
    Ok(finalize_specs(
        specs,
        selections_before.clone(),
        selections_before,
    ))
}

pub(crate) fn plan_markdown_cycle_list_marker(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let lines = lines_in(buffer);
    let mut specs = Vec::new();
    for &line in &lines {
        let start = rope.line_to_byte(line);
        let end = line_content_end(rope, line);
        let text = rope.byte_slice(start..end).to_string();
        let leading = leading_whitespace_len(&text);
        let body = &text[leading..];
        let next_marker = body
            .strip_prefix("- ")
            .map(|rest| ("* ", rest))
            .or_else(|| body.strip_prefix("* ").map(|rest| ("+ ", rest)))
            .or_else(|| body.strip_prefix("+ ").map(|rest| ("- ", rest)));
        if let Some((marker, rest)) = next_marker {
            let replacement = format!("{}{marker}{rest}", &text[..leading]);
            specs.push(EditSpec::replace(rope, start, end, replacement)?);
        }
    }
    Ok(finalize_specs(
        specs,
        selections_before.clone(),
        selections_before,
    ))
}

pub(crate) fn plan_markdown_wrap_in_blockquote(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    toggle_line_prefix(buffer, &["> "], "> ")
}

pub(crate) fn plan_markdown_insert_code_fence(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let mut specs = Vec::new();
    let mut selections_after = Vec::new();
    for selection in &selections_before {
        let range = selection.ordered_range();
        let start = range.start.to_byte_offset(rope)?;
        let end = range.end.to_byte_offset(rope)?;
        if start == end {
            // Insert an empty fence with caret between the lines.
            let inserted = "```\n\n```\n".to_string();
            specs.push(EditSpec::insert(rope, start, inserted.clone())?);
            let head = advance_position(range.start, "```\n");
            selections_after.push(Selection::caret_at(head));
        } else {
            let original = rope.byte_slice(start..end).to_string();
            let replaced = format!("```\n{original}\n```");
            specs.push(EditSpec::replace(rope, start, end, replaced.clone())?);
            let new_head = advance_position(range.start, &replaced);
            selections_after.push(Selection::new(range.start, new_head, selection.kind));
        }
    }
    Ok(finalize_specs(specs, selections_before, selections_after))
}

pub(crate) fn plan_markdown_insert_link(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    insert_around(buffer, "[", "](url)", "[text](url)")
}

pub(crate) fn plan_markdown_insert_image_ref(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    insert_around(buffer, "![", "](path)", "![alt](path)")
}

fn toggle_line_prefix(
    buffer: &Buffer,
    strip_candidates: &[&str],
    add_prefix: &str,
) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let lines = lines_in(buffer);
    let mut specs = Vec::new();
    let all_have_prefix = lines.iter().all(|&line| {
        let text = line_text(rope, line);
        let leading = leading_whitespace_len(&text);
        let body = &text[leading..];
        strip_candidates.iter().any(|c| body.starts_with(*c))
    });
    for &line in &lines {
        let start = rope.line_to_byte(line);
        let end = line_content_end(rope, line);
        let text = rope.byte_slice(start..end).to_string();
        let leading = leading_whitespace_len(&text);
        let body = &text[leading..];
        let replacement = if all_have_prefix {
            for candidate in strip_candidates {
                if let Some(rest) = body.strip_prefix(*candidate) {
                    let r = format!("{}{rest}", &text[..leading]);
                    specs.push(EditSpec::replace(rope, start, end, r)?);
                    break;
                }
            }
            continue;
        } else {
            format!("{}{add_prefix}{body}", &text[..leading])
        };
        specs.push(EditSpec::replace(rope, start, end, replacement)?);
    }
    Ok(finalize_specs(
        specs,
        selections_before.clone(),
        selections_before,
    ))
}

fn insert_around(
    buffer: &Buffer,
    open: &str,
    close: &str,
    fallback: &str,
) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let mut specs = Vec::new();
    let mut selections_after = Vec::new();
    for selection in &selections_before {
        let range = selection.ordered_range();
        let start = range.start.to_byte_offset(rope)?;
        let end = range.end.to_byte_offset(rope)?;
        if start == end {
            specs.push(EditSpec::insert(rope, start, fallback.to_string())?);
            let head = advance_position(range.start, fallback);
            selections_after.push(Selection::caret_at(head));
        } else {
            let original = rope.byte_slice(start..end).to_string();
            let replaced = format!("{open}{original}{close}");
            specs.push(EditSpec::replace(rope, start, end, replaced.clone())?);
            let new_head = advance_position(range.start, &replaced);
            selections_after.push(Selection::new(range.start, new_head, SelectionKind::Caret));
        }
    }
    Ok(finalize_specs(specs, selections_before, selections_after))
}

/// Sorted, deduplicated list of line numbers covered by the buffer's
/// selections, clipped to the rope length. Shared with
/// `edit_markdown_blocks.rs`.
pub(crate) fn lines_in(buffer: &Buffer) -> Vec<usize> {
    let mut out = Vec::new();
    let len_lines = buffer.rope().len_lines();
    for selection in buffer.selections() {
        let range = selection.ordered_range();
        for line in (range.start.line as usize)..=(range.end.line as usize) {
            if line < len_lines && !out.contains(&line) {
                out.push(line);
            }
        }
    }
    out.sort_unstable();
    out
}

fn emphasis_delim(kind: EmphasisKind) -> (&'static str, &'static str) {
    match kind {
        EmphasisKind::Bold => ("**", "**"),
        EmphasisKind::Italic => ("*", "*"),
        EmphasisKind::Strikethrough => ("~~", "~~"),
        EmphasisKind::InlineCode => ("`", "`"),
    }
}

fn strip_wrap<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    if text.starts_with(open) && text.ends_with(close) && text.len() >= open.len() + close.len() {
        Some(&text[open.len()..text.len() - close.len()])
    } else {
        None
    }
}

/// Heading level (1..=6) of a single line, or `0` when the line is not a
/// markdown ATX heading. Shared with `edit_markdown_blocks.rs`.
pub(crate) fn heading_level(text: &str) -> u8 {
    let mut level = 0u8;
    for c in text.chars().take(7) {
        if c == '#' && level < 6 {
            level += 1;
        } else if c == ' ' && level > 0 {
            return level;
        } else {
            return 0;
        }
    }
    0
}

/// Strip the `#`-prefix and following space from an ATX heading line. When
/// the line is not a heading, returns the original text. Shared with
/// `edit_markdown_blocks.rs`.
pub(crate) fn strip_heading_prefix(text: &str) -> &str {
    let mut idx = 0;
    let bytes = text.as_bytes();
    while idx < bytes.len() && bytes[idx] == b'#' && idx < 6 {
        idx += 1;
    }
    if idx == 0 {
        return text;
    }
    if idx < bytes.len() && bytes[idx] == b' ' {
        return &text[idx + 1..];
    }
    text
}

fn leading_whitespace_len(text: &str) -> usize {
    text.chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .map(char::len_utf8)
        .sum()
}

fn strip_numbered_prefix(body: &str) -> Option<&str> {
    let digit_count = body.chars().take_while(|c| c.is_ascii_digit()).count();
    if digit_count == 0 {
        return None;
    }
    let rest = &body[digit_count..];
    rest.strip_prefix(". ")
}

/// Read a single line's content (without trailing newline) as an owned
/// string. Shared with `edit_markdown_blocks.rs`.
pub(crate) fn line_text(rope: &Rope, line: usize) -> String {
    let start = rope.line_to_byte(line);
    let end = line_content_end(rope, line);
    rope.byte_slice(start..end).to_string()
}

/// Walk upward from `line` until we hit an ATX heading; return its line
/// index. Shared with `edit_markdown_blocks.rs`.
pub(crate) fn enclosing_heading_line(rope: &Rope, line: usize) -> Option<usize> {
    let mut probe = line;
    loop {
        let text = line_text(rope, probe);
        if heading_level(&text) > 0 {
            return Some(probe);
        }
        if probe == 0 {
            return None;
        }
        probe -= 1;
    }
}

/// Walk upward from `before_inclusive` to find the previous heading line.
/// Shared with `edit_markdown_blocks.rs`.
pub(crate) fn previous_section_start(rope: &Rope, before_inclusive: usize) -> Option<usize> {
    let mut probe = before_inclusive;
    loop {
        if heading_level(&line_text(rope, probe)) > 0 {
            return Some(probe);
        }
        if probe == 0 {
            return None;
        }
        probe -= 1;
    }
}

/// If the line at `line` is a heading, return its level; otherwise `None`.
/// Shared with `edit_markdown_blocks.rs`.
pub(crate) fn next_heading_level(rope: &Rope, line: usize) -> Option<u8> {
    let lvl = heading_level(&line_text(rope, line));
    if lvl > 0 {
        Some(lvl)
    } else {
        None
    }
}

/// Last line of the section starting at `start` of given `level` —
/// extends until the next heading at the same or lower level (or EOF).
/// Shared with `edit_markdown_blocks.rs`.
pub(crate) fn section_end_line(rope: &Rope, start: usize, level: u8) -> usize {
    let mut last = start;
    let len = rope.len_lines();
    for line in (start + 1)..len {
        let lvl = heading_level(&line_text(rope, line));
        if lvl > 0 && lvl <= level {
            return last;
        }
        last = line;
    }
    last
}

#[cfg(test)]
mod tests {
    use continuity_buffer::Buffer;
    use continuity_text::{Position, Selection, SelectionKind};

    use super::*;
    use crate::selection_edit::{apply_plan, plan, SelectionEdit};

    fn caret(line: u32, col: u32) -> Selection {
        Selection::caret_at(Position::new(line, col))
    }

    fn span(start: (u32, u32), end: (u32, u32)) -> Selection {
        Selection::new(
            Position::new(start.0, start.1),
            Position::new(end.0, end.1),
            SelectionKind::Caret,
        )
    }

    fn run(buffer: &mut Buffer, edit: SelectionEdit) {
        let plan = plan(buffer, &edit).expect("plan ok").expect("plan some");
        apply_plan(buffer, &plan).expect("apply ok");
    }

    #[test]
    fn toggle_bold_wraps_selection() {
        let mut b = Buffer::from_text("text");
        b.set_selections(vec![span((0, 0), (0, 4))]);
        run(
            &mut b,
            SelectionEdit::MarkdownToggleEmphasis(EmphasisKind::Bold),
        );
        assert_eq!(b.rope().to_string(), "**text**");
    }

    #[test]
    fn toggle_bold_multiline_wraps_each_line() {
        let mut b = Buffer::from_text("foo\nbar");
        b.set_selections(vec![span((0, 0), (1, 3))]);
        run(
            &mut b,
            SelectionEdit::MarkdownToggleEmphasis(EmphasisKind::Bold),
        );
        assert_eq!(b.rope().to_string(), "**foo**\n**bar**");
    }

    #[test]
    fn toggle_bold_multiline_preserves_indent_and_blank_lines() {
        let mut b = Buffer::from_text("  foo\n\n  bar");
        b.set_selections(vec![span((0, 0), (2, 5))]);
        run(
            &mut b,
            SelectionEdit::MarkdownToggleEmphasis(EmphasisKind::Bold),
        );
        // Leading whitespace stays outside the markers; blank line stays
        // blank — keeps the parser happy and the source readable.
        assert_eq!(b.rope().to_string(), "  **foo**\n\n  **bar**");
    }

    #[test]
    fn toggle_bold_strips_existing_wrap() {
        let mut b = Buffer::from_text("**text**");
        b.set_selections(vec![span((0, 0), (0, 8))]);
        run(
            &mut b,
            SelectionEdit::MarkdownToggleEmphasis(EmphasisKind::Bold),
        );
        assert_eq!(b.rope().to_string(), "text");
    }

    #[test]
    fn toggle_bullet_adds_then_removes() {
        let mut b = Buffer::from_text("a");
        b.set_selections(vec![caret(0, 0)]);
        run(&mut b, SelectionEdit::MarkdownToggleBullet);
        assert_eq!(b.rope().to_string(), "- a");
        run(&mut b, SelectionEdit::MarkdownToggleBullet);
        assert_eq!(b.rope().to_string(), "a");
    }

    #[test]
    fn toggle_numbered_inserts_one_dot_space() {
        let mut b = Buffer::from_text("a\nb");
        b.set_selections(vec![span((0, 0), (1, 1))]);
        run(&mut b, SelectionEdit::MarkdownToggleNumbered);
        assert_eq!(b.rope().to_string(), "1. a\n2. b");
    }

    #[test]
    fn toggle_checkbox_cycles() {
        let mut b = Buffer::from_text("a");
        b.set_selections(vec![caret(0, 0)]);
        run(&mut b, SelectionEdit::MarkdownToggleCheckbox);
        assert_eq!(b.rope().to_string(), "[ ] a");
        run(&mut b, SelectionEdit::MarkdownToggleCheckbox);
        assert_eq!(b.rope().to_string(), "[x] a");
    }

    #[test]
    fn cycle_list_marker_advances() {
        let mut b = Buffer::from_text("- a");
        b.set_selections(vec![caret(0, 0)]);
        run(&mut b, SelectionEdit::MarkdownCycleListMarker);
        assert_eq!(b.rope().to_string(), "* a");
    }

    #[test]
    fn wrap_in_blockquote_prefixes() {
        let mut b = Buffer::from_text("a");
        b.set_selections(vec![caret(0, 0)]);
        run(&mut b, SelectionEdit::MarkdownWrapInBlockquote);
        assert_eq!(b.rope().to_string(), "> a");
    }

    #[test]
    fn insert_code_fence_wraps_selection() {
        let mut b = Buffer::from_text("x");
        b.set_selections(vec![span((0, 0), (0, 1))]);
        run(&mut b, SelectionEdit::MarkdownInsertCodeFence);
        assert_eq!(b.rope().to_string(), "```\nx\n```");
    }

    #[test]
    fn insert_link_wraps_selection() {
        let mut b = Buffer::from_text("hi");
        b.set_selections(vec![span((0, 0), (0, 2))]);
        run(&mut b, SelectionEdit::MarkdownInsertLink);
        assert_eq!(b.rope().to_string(), "[hi](url)");
    }

    #[test]
    fn insert_image_ref_at_caret() {
        let mut b = Buffer::from_text("");
        b.set_selections(vec![caret(0, 0)]);
        run(&mut b, SelectionEdit::MarkdownInsertImageRef);
        assert_eq!(b.rope().to_string(), "![alt](path)");
    }
}
