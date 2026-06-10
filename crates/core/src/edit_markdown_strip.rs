//! Strip-all-markdown-formatting planner (`markdown.strip_formatting`).
//!
//! Rewrites every line covered by the selections with its markdown
//! syntax removed: blockquote prefixes, heading hashes, list markers
//! (bullet / ordered / task checkbox), emphasis / code / strikethrough
//! delimiters, and link / image syntax (keeping the visible text).
//!
//! String-based like the rest of `edit_markdown*` — deliberately
//! conservative on inline delimiters: only *paired* delimiters whose
//! inner edges are non-whitespace are removed, and `_` pairs must sit
//! on word boundaries, so `snake_case_names` and `2 * 3 * 4` survive.

use continuity_buffer::Buffer;
use continuity_text::{Position, Selection};

use crate::edit_markdown::{leading_whitespace_len, lines_in, split_leading_list_marker};
use crate::edit_planning::{finalize_specs, line_content_end, EditSpec};
use crate::selection_edit::SelectionEditPlan;
use crate::Error;

pub(crate) fn plan_markdown_strip_formatting(
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
        let stripped = strip_line_markdown(&text);
        if stripped != text {
            specs.push(EditSpec::replace(rope, start, end, stripped)?);
        }
    }
    // Stripping can shorten lines arbitrarily, so stale byte columns
    // would dangle; land a caret at the start of each selection's first
    // covered line instead.
    let selections_after = selections_before
        .iter()
        .map(|sel| Selection::caret_at(Position::new(sel.ordered_range().start.line, 0)))
        .collect();
    Ok(finalize_specs(specs, selections_before, selections_after))
}

/// One line, markdown syntax removed; leading whitespace preserved.
fn strip_line_markdown(line: &str) -> String {
    let leading = leading_whitespace_len(line);
    let (indent, mut body) = line.split_at(leading);
    // Blockquote prefixes, possibly nested (`> > text`).
    loop {
        if let Some(rest) = body.strip_prefix("> ") {
            body = rest;
        } else if body == ">" {
            body = "";
        } else {
            break;
        }
    }
    // Heading hashes.
    body = strip_heading_hashes(body);
    // List marker (`- ` / `* ` / `+ ` / `N. ` / `N) `) + optional task
    // checkbox after it.
    let (marker, mut after) = split_leading_list_marker(body);
    if !marker.is_empty() {
        for checkbox in ["[ ] ", "[x] ", "[X] "] {
            if let Some(rest) = after.strip_prefix(checkbox) {
                after = rest;
                break;
            }
        }
        body = after;
    }
    format!("{indent}{}", strip_inline_markdown(body))
}

/// `## text` → `text`. Unlike `strip_heading_prefix` this also accepts
/// hash runs longer than six (treated as plain text by CommonMark, but
/// the writer asking to strip formatting wants them gone regardless).
fn strip_heading_hashes(body: &str) -> &str {
    let hashes = body.bytes().take_while(|b| *b == b'#').count();
    if hashes == 0 {
        return body;
    }
    match body.as_bytes().get(hashes) {
        Some(b' ') => &body[hashes + 1..],
        None => "",
        _ => body,
    }
}

fn strip_inline_markdown(text: &str) -> String {
    let mut out = strip_links(text);
    for delim in ["**", "__", "~~", "`", "*", "_"] {
        out = strip_paired_delimiter(&out, delim);
    }
    out
}

/// `[label](target)` → `label`, `![alt](target)` → `alt`.
fn strip_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let Some(bracket) = rest.find('[') else {
            out.push_str(rest);
            return out;
        };
        let is_image = rest[..bracket].ends_with('!');
        let prefix_end = if is_image { bracket - 1 } else { bracket };
        let Some((label, after)) = parse_link_at(rest, bracket) else {
            out.push_str(&rest[..bracket + 1]);
            rest = &rest[bracket + 1..];
            continue;
        };
        out.push_str(&rest[..prefix_end]);
        out.push_str(label);
        rest = &rest[after..];
    }
}

/// Parse `[label](target)` starting at `open_bracket`. Returns the
/// label slice and the byte offset just past the closing paren.
fn parse_link_at(text: &str, open_bracket: usize) -> Option<(&str, usize)> {
    let close_bracket = text[open_bracket + 1..]
        .find(']')
        .map(|o| open_bracket + 1 + o)?;
    let after_bracket = close_bracket + 1;
    if !text[after_bracket..].starts_with('(') {
        return None;
    }
    let close_paren = text[after_bracket + 1..]
        .find(')')
        .map(|o| after_bracket + 1 + o)?;
    Some((&text[open_bracket + 1..close_bracket], close_paren + 1))
}

/// Remove paired `delim` occurrences whose inner text is non-empty and
/// whose inner edges are non-whitespace. `_`-family pairs additionally
/// require word boundaries outside the pair, so intraword underscores
/// (`snake_case`) survive.
fn strip_paired_delimiter(text: &str, delim: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text.to_string();
    loop {
        let Some(first) = rest.find(delim) else {
            out.push_str(&rest);
            return out;
        };
        let after_first = first + delim.len();
        let Some(second_rel) = rest[after_first..].find(delim) else {
            out.push_str(&rest);
            return out;
        };
        let second = after_first + second_rel;
        let inner = &rest[after_first..second];
        let inner_ok = !inner.is_empty()
            && !inner.starts_with(|c: char| c.is_whitespace())
            && !inner.ends_with(|c: char| c.is_whitespace());
        let boundary_ok = if delim.starts_with('_') {
            let before_ok = rest[..first]
                .chars()
                .last()
                .is_none_or(|c| !c.is_alphanumeric());
            let after_ok = rest[second + delim.len()..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_alphanumeric());
            before_ok && after_ok
        } else {
            true
        };
        if inner_ok && boundary_ok {
            out.push_str(&rest[..first]);
            out.push_str(inner);
            rest = rest[second + delim.len()..].to_string();
        } else {
            out.push_str(&rest[..after_first]);
            rest = rest[after_first..].to_string();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection_edit::{apply_plan, plan, SelectionEdit};
    use continuity_text::SelectionKind;

    fn run_strip(text: &str) -> String {
        let mut b = Buffer::from_text(text);
        let last_line = text.split('\n').count() as u32 - 1;
        let last_len = text.split('\n').next_back().unwrap_or("").len() as u32;
        b.set_selections(vec![Selection::new(
            Position::new(0, 0),
            Position::new(last_line, last_len),
            SelectionKind::Caret,
        )]);
        let plan = plan(&b, &SelectionEdit::MarkdownStripFormatting)
            .expect("plan ok")
            .expect("plan some");
        apply_plan(&mut b, &plan).expect("apply ok");
        b.rope().to_string()
    }

    #[test]
    fn strips_headings_bullets_and_emphasis() {
        assert_eq!(
            run_strip("# Title\n- **bold** item\n2. *it* works"),
            "Title\nbold item\nit works"
        );
    }

    #[test]
    fn strips_task_checkbox_blockquote_and_code() {
        assert_eq!(
            run_strip("- [x] `done` task\n> quoted ~~old~~ text"),
            "done task\nquoted old text"
        );
    }

    #[test]
    fn strips_links_keeping_text() {
        assert_eq!(
            run_strip("see [the docs](https://example.com) and ![alt](img.png)"),
            "see the docs and alt"
        );
    }

    #[test]
    fn preserves_indent_snake_case_and_arithmetic() {
        assert_eq!(
            run_strip("  - keep_snake_case and 2 * 3 * 4"),
            "  keep_snake_case and 2 * 3 * 4"
        );
    }

    #[test]
    fn plain_text_is_a_no_op() {
        let mut b = Buffer::from_text("nothing to strip");
        b.set_selections(vec![Selection::new(
            Position::new(0, 0),
            Position::new(0, 16),
            SelectionKind::Caret,
        )]);
        let plan = plan(&b, &SelectionEdit::MarkdownStripFormatting).expect("plan ok");
        assert!(plan.is_none(), "no edits → no undo group");
    }
}
