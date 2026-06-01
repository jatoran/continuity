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

/// Split an optional leading list marker (`- `, `* `, `+ `, `N. `, `N) `)
/// off the front of a line body. Returns `(marker, rest)`; `marker` is
/// empty when the body has no marker. Lets checkbox/task toggles operate
/// *after* the bullet instead of mistaking `- [ ] x` for plain text.
fn split_leading_list_marker(body: &str) -> (&str, &str) {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = body.strip_prefix(marker) {
            return (&body[..marker.len()], rest);
        }
    }
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i > 0
        && i + 1 < bytes.len()
        && (bytes[i] == b'.' || bytes[i] == b')')
        && bytes[i + 1] == b' '
    {
        return (&body[..i + 2], &body[i + 2..]);
    }
    ("", body)
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
        let indent = &text[..leading];
        let body = &text[leading..];
        // A checkbox can sit *after* a list marker (`- [ ] `). Split the
        // marker off first; otherwise the toggle never finds the existing
        // checkbox on a `- [ ] ` line and prepends a duplicate
        // (`[ ] - [ ] …`).
        let (marker, after) = split_leading_list_marker(body);
        let replacement = if let Some(rest) = after.strip_prefix("[ ] ") {
            format!("{indent}{marker}[x] {rest}")
        } else if let Some(rest) = after
            .strip_prefix("[x] ")
            .or_else(|| after.strip_prefix("[X] "))
        {
            format!("{indent}{marker}[ ] {rest}")
        } else {
            format!("{indent}{marker}[ ] {after}")
        };
        specs.push(EditSpec::replace(rope, start, end, replacement)?);
    }
    Ok(finalize_specs(
        specs,
        selections_before.clone(),
        selections_before,
    ))
}

/// Toggle a full `- [ ] ` task bullet on each covered line. A line that
/// is already a task (`- [ ] `, `[x] `, …) loses the whole prefix; a
/// plain bullet (`- foo`) keeps its marker and gains a checkbox; any
/// other line is prefixed with `- [ ] `. Bound to `Ctrl+E`.
pub(crate) fn plan_markdown_toggle_task(
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
        let indent = &text[..leading];
        let body = &text[leading..];
        let (marker, after) = split_leading_list_marker(body);
        let is_task =
            after.starts_with("[ ] ") || after.starts_with("[x] ") || after.starts_with("[X] ");
        let replacement = if is_task {
            // Drop the marker + checkbox, leaving plain content.
            format!("{indent}{}", &after[4..])
        } else if !marker.is_empty() {
            // Plain bullet → task bullet, keeping the existing marker.
            format!("{indent}{marker}[ ] {after}")
        } else {
            // Plain line → task bullet.
            format!("{indent}- [ ] {after}")
        };
        specs.push(EditSpec::replace(rope, start, end, replacement)?);
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
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let mut specs = Vec::new();
    let mut selections_after = Vec::new();
    for selection in &selections_before {
        let range = selection.ordered_range();
        let start = range.start.to_byte_offset(rope)?;
        let end = range.end.to_byte_offset(rope)?;
        let sel_text = rope.byte_slice(start..end).to_string();
        // Drop the caret in the spot the writer still needs to fill:
        //   selection is a URL  -> `[|](url)`  (type the visible label)
        //   selection is text   -> `[text](|)` (type / paste the URL)
        //   nothing selected    -> `[|]()`     (type the visible label)
        let (replacement, caret_prefix_len) = if start == end {
            ("[]()".to_string(), 1)
        } else if selection_looks_like_url(&sel_text) {
            (format!("[]({})", sel_text.trim()), 1)
        } else {
            // Caret after `[<text>](`.
            (format!("[{sel_text}]()"), 1 + sel_text.len() + 2)
        };
        if start == end {
            specs.push(EditSpec::insert(rope, start, replacement.clone())?);
        } else {
            specs.push(EditSpec::replace(rope, start, end, replacement.clone())?);
        }
        let head = advance_position(range.start, &replacement[..caret_prefix_len]);
        selections_after.push(Selection::caret_at(head));
    }
    Ok(finalize_specs(specs, selections_before, selections_after))
}

/// Heuristic: does the selected text look like a URL the writer wants as
/// the link *target* (rather than the visible label)? Drives which
/// bracket the caret lands in for [`plan_markdown_insert_link`].
fn selection_looks_like_url(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() || s.split_whitespace().count() != 1 {
        return false;
    }
    if s.contains("://")
        || s.starts_with("www.")
        || s.starts_with("mailto:")
        || s.starts_with("tel:")
    {
        return true;
    }
    // Bare domain like `example.com` or `a.b/path`: a dotted host with a
    // non-empty label and an alphabetic TLD of length >= 2.
    let host = s.split(['/', '?', '#']).next().unwrap_or(s);
    if let Some(dot) = host.rfind('.') {
        let before = &host[..dot];
        let tld = &host[dot + 1..];
        return !before.is_empty()
            && tld.len() >= 2
            && tld.chars().all(|c| c.is_ascii_alphabetic());
    }
    false
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
mod tests;
