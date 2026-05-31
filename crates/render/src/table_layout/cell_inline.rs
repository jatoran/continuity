//! Inline markdown parsing for one pipe-table cell.
//!
//! Pipe-table cells live outside the standard block-level decoration
//! pipeline (the display map flattens pipes, the renderer rebuilds the
//! visual cells from raw rope bytes). This module is the cell-local
//! equivalent of `decorate::inline_text` — given a cell's trimmed raw
//! text, it returns the **display text** with bold/italic/strike/code
//! markers stripped, plus a parallel list of style runs whose byte
//! ranges index into the returned display text.
//!
//! The parser is intentionally narrow: single-level bold (`**…**` or
//! `__…__`), single-level italic (`*…*` / `_…_`), inline code
//! (`` `…` ``), strike (`~~…~~`), and link text (`[text](url)` — the
//! `[`, `](url)` markers strip, the inner text shows). Nesting beyond
//! "code inside emphasis" is uncommon in tables; the parser handles
//! the no-nesting case greedily and falls back to literal copy on
//! anything ambiguous.
//!
//! Thread ownership: pure data, callable from any thread.

use std::ops::Range;

use continuity_display_map::SpanStyle;

/// Result of parsing one cell's raw text.
///
/// `display_text` is what the painter draws; `inline_runs` is the
/// per-byte styling indexed into `display_text` (NOT the source).
/// Empty cells return empty display + empty runs.
pub(super) struct CellInline {
    pub display_text: String,
    pub inline_runs: Vec<(Range<u32>, SpanStyle)>,
}

/// Parse `raw` (a trimmed cell payload) into display text + style runs.
///
/// When the cell carries no markdown markers, the result's display text
/// equals `raw` and `inline_runs` is empty — the painter then renders
/// the cell as plain text exactly as before Phase B.
pub(super) fn compute_cell_inline(raw: &str) -> CellInline {
    let bytes = raw.as_bytes();
    let mut display = String::with_capacity(raw.len());
    let mut runs: Vec<(Range<u32>, SpanStyle)> = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        // Inline code: backtick run; consume until a matching-length
        // closing run on the same line. Cell text never spans lines
        // (table rows are single-line), so no newline handling needed.
        if b == b'`' {
            let open_start = i;
            let mut open_count = 0usize;
            while i < bytes.len() && bytes[i] == b'`' {
                i += 1;
                open_count += 1;
            }
            let body_start = i;
            let mut found = None;
            while i < bytes.len() {
                if bytes[i] == b'`' {
                    let run_start = i;
                    let mut run = 0usize;
                    while i < bytes.len() && bytes[i] == b'`' {
                        i += 1;
                        run += 1;
                    }
                    if run == open_count {
                        found = Some((run_start, i));
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            if let Some((close_start, close_end)) = found {
                let body = &raw[body_start..close_start];
                let display_start = display.len() as u32;
                display.push_str(body);
                let display_end = display.len() as u32;
                if display_end > display_start {
                    runs.push((display_start..display_end, SpanStyle::code()));
                }
                let _ = close_end;
                continue;
            }
            // Unmatched — emit the opening ticks literally and rewind
            // `i` to the first body byte so the rest of the cell text
            // is re-walked as regular content. Without the rewind we
            // already consumed every body byte during the search-for-
            // matching-close scan, and the body would never appear.
            display.push_str(&raw[open_start..body_start]);
            i = body_start;
            continue;
        }
        // Strike: `~~…~~`.
        if b == b'~' && i + 1 < bytes.len() && bytes[i + 1] == b'~' {
            let body_start = i + 2;
            let mut j = body_start;
            let mut close = None;
            while j + 1 < bytes.len() {
                if bytes[j] == b'~' && bytes[j + 1] == b'~' {
                    close = Some(j);
                    break;
                }
                j += 1;
            }
            if let Some(close_start) = close {
                let body = &raw[body_start..close_start];
                let inner = compute_cell_inline(body);
                let display_start = display.len() as u32;
                display.push_str(&inner.display_text);
                let display_end = display.len() as u32;
                if display_end > display_start {
                    runs.push((display_start..display_end, SpanStyle::strike()));
                }
                let base = display_start;
                for (range, style) in inner.inline_runs {
                    runs.push((merge_strike(range, base), style));
                }
                i = close_start + 2;
                continue;
            }
            // Unmatched — emit one `~` literally and resume.
            display.push('~');
            i += 1;
            continue;
        }
        // Bold (`**…**` / `__…__`) and italic (`*…*` / `_…_`).
        if b == b'*' || b == b'_' {
            if let Some((kind, body_start, body_end, end)) = parse_emphasis(bytes, i) {
                let body = &raw[body_start..body_end];
                let inner = compute_cell_inline(body);
                let display_start = display.len() as u32;
                display.push_str(&inner.display_text);
                let display_end = display.len() as u32;
                if display_end > display_start {
                    let style = match kind {
                        EmphasisKind::Strong => SpanStyle::strong(),
                        EmphasisKind::Emphasis => SpanStyle::emphasis(),
                    };
                    runs.push((display_start..display_end, style));
                }
                for (range, style) in inner.inline_runs {
                    runs.push((merge_strike(range, display_start), style));
                }
                i = end;
                continue;
            }
        }
        // Link: `[text](url)` — display `text` styled as link.
        if b == b'[' {
            if let Some((text_start, text_end, end)) = parse_link(bytes, i) {
                let body = &raw[text_start..text_end];
                let inner = compute_cell_inline(body);
                let display_start = display.len() as u32;
                display.push_str(&inner.display_text);
                let display_end = display.len() as u32;
                if display_end > display_start {
                    runs.push((display_start..display_end, SpanStyle::link()));
                }
                for (range, style) in inner.inline_runs {
                    runs.push((merge_strike(range, display_start), style));
                }
                i = end;
                continue;
            }
        }
        // Escaped pipe `\|` shows as `|` in cell.
        if b == b'\\' && i + 1 < bytes.len() && bytes[i + 1] == b'|' {
            display.push('|');
            i += 2;
            continue;
        }
        // Default: copy one UTF-8 char to display.
        let char_end = next_utf8_boundary(bytes, i);
        display.push_str(&raw[i..char_end]);
        i = char_end;
    }
    CellInline {
        display_text: display,
        inline_runs: runs,
    }
}

#[derive(Copy, Clone)]
enum EmphasisKind {
    Strong,
    Emphasis,
}

/// Try to parse a `*…*` / `**…**` / `_…_` / `__…__` run starting at
/// `start`. Returns `(kind, body_start, body_end, end_after_close)`.
///
/// Greedy on opener width: two markers → Strong, one → Emphasis. Falls
/// back to `None` when the run doesn't close on the same cell, which
/// causes the caller to render the marker as a literal character.
fn parse_emphasis(bytes: &[u8], start: usize) -> Option<(EmphasisKind, usize, usize, usize)> {
    let ch = bytes[start];
    if ch != b'*' && ch != b'_' {
        return None;
    }
    let mut open_count = 0usize;
    let mut i = start;
    while i < bytes.len() && bytes[i] == ch && open_count < 2 {
        i += 1;
        open_count += 1;
    }
    if open_count == 0 {
        return None;
    }
    if i >= bytes.len() || matches!(bytes[i], b' ' | b'\t') {
        return None;
    }
    let body_start = i;
    let mut j = body_start;
    while j < bytes.len() {
        if bytes[j] == ch && j > body_start && !matches!(bytes[j - 1], b' ' | b'\t') {
            let run_start = j;
            let mut run = 0usize;
            while j < bytes.len() && bytes[j] == ch && run < open_count {
                j += 1;
                run += 1;
            }
            if run == open_count {
                let kind = if open_count == 2 {
                    EmphasisKind::Strong
                } else {
                    EmphasisKind::Emphasis
                };
                return Some((kind, body_start, run_start, j));
            }
            continue;
        }
        j += 1;
    }
    None
}

/// Try to parse `[text](url)` starting at `lbrack` (the `[` byte).
/// Returns `(text_start, text_end, end_after_close_paren)` so the
/// caller can splice the rendered text into the display buffer.
fn parse_link(bytes: &[u8], lbrack: usize) -> Option<(usize, usize, usize)> {
    if lbrack >= bytes.len() || bytes[lbrack] != b'[' {
        return None;
    }
    let text_start = lbrack + 1;
    let mut depth = 1i32;
    let mut i = text_start;
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b'[' => depth += 1,
            b']' => depth -= 1,
            b'\n' => return None,
            _ => {}
        }
        if depth == 0 {
            break;
        }
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b']' {
        return None;
    }
    let text_end = i;
    if i + 1 >= bytes.len() || bytes[i + 1] != b'(' {
        return None;
    }
    i += 2;
    while i < bytes.len() && bytes[i] != b')' && bytes[i] != b'\n' {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b')' {
        return None;
    }
    Some((text_start, text_end, i + 1))
}

/// Shift an inner-content run range by `base` so it indexes into the
/// outer display buffer. Inner runs were emitted assuming the inner
/// display string started at 0; the outer buffer already wrote `base`
/// bytes of preceding content before splicing in the inner text.
fn merge_strike(range: Range<u32>, base: u32) -> Range<u32> {
    (range.start + base)..(range.end + base)
}

/// Advance one UTF-8 character starting at `i`. Returns the index past
/// the character's last byte. Safe for any valid UTF-8 input.
fn next_utf8_boundary(bytes: &[u8], i: usize) -> usize {
    let first = bytes[i];
    let len = if first < 0x80 {
        1
    } else if first < 0xC0 {
        // Continuation byte by itself — shouldn't happen in valid
        // UTF-8 but advance one byte rather than spin.
        1
    } else if first < 0xE0 {
        2
    } else if first < 0xF0 {
        3
    } else {
        4
    };
    (i + len).min(bytes.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_display_map::SpanRole;

    fn parse(raw: &str) -> CellInline {
        compute_cell_inline(raw)
    }

    #[test]
    fn plain_text_passes_through_unchanged() {
        let r = parse("hello world");
        assert_eq!(r.display_text, "hello world");
        assert!(r.inline_runs.is_empty());
    }

    #[test]
    fn bold_strips_markers_and_emits_strong_run() {
        let r = parse("**bold** plain");
        assert_eq!(r.display_text, "bold plain");
        let strong = r
            .inline_runs
            .iter()
            .find(|(_, s)| s.bold)
            .expect("should have a bold run");
        assert_eq!(strong.0, 0..4);
    }

    #[test]
    fn italic_strips_single_marker() {
        let r = parse("plain *italic* word");
        assert_eq!(r.display_text, "plain italic word");
        let emph = r
            .inline_runs
            .iter()
            .find(|(_, s)| s.italic && !s.bold)
            .expect("should have an italic run");
        assert_eq!(emph.0, 6..12);
    }

    #[test]
    fn inline_code_strips_backticks_and_marks_code_role() {
        let r = parse("see `let x = 1` here");
        assert_eq!(r.display_text, "see let x = 1 here");
        let code = r
            .inline_runs
            .iter()
            .find(|(_, s)| matches!(s.role, SpanRole::Code))
            .expect("should have a code run");
        assert_eq!(code.0, 4..13);
    }

    #[test]
    fn strike_strips_tildes() {
        let r = parse("~~gone~~ kept");
        assert_eq!(r.display_text, "gone kept");
        let strike = r
            .inline_runs
            .iter()
            .find(|(_, s)| s.strikethrough)
            .expect("should have a strike run");
        assert_eq!(strike.0, 0..4);
    }

    #[test]
    fn link_strips_brackets_and_url() {
        let r = parse("see [docs](https://x.com) for more");
        assert_eq!(r.display_text, "see docs for more");
        let link = r
            .inline_runs
            .iter()
            .find(|(_, s)| matches!(s.role, SpanRole::Link))
            .expect("should have a link run");
        assert_eq!(link.0, 4..8);
    }

    #[test]
    fn unmatched_emphasis_renders_as_literal() {
        let r = parse("a*b");
        assert_eq!(r.display_text, "a*b");
        assert!(r.inline_runs.is_empty());
    }

    #[test]
    fn unmatched_code_renders_as_literal() {
        let r = parse("`unclosed");
        assert_eq!(r.display_text, "`unclosed");
    }

    #[test]
    fn underscore_bold_strips_double_marker() {
        let r = parse("__strong__");
        assert_eq!(r.display_text, "strong");
        assert!(r.inline_runs.iter().any(|(_, s)| s.bold));
    }

    #[test]
    fn escaped_pipe_becomes_literal_pipe() {
        let r = parse(r"a\|b");
        assert_eq!(r.display_text, "a|b");
    }

    #[test]
    fn nested_bold_in_italic_emits_both_runs() {
        // `_outer **inner** end_` → display "outer inner end", italic
        // over the whole span plus bold over just `inner`.
        let r = parse("_outer **inner** end_");
        assert_eq!(r.display_text, "outer inner end");
        assert!(r.inline_runs.iter().any(|(_, s)| s.italic && !s.bold));
        assert!(r.inline_runs.iter().any(|(_, s)| s.bold));
        // Bold range should sit inside the italic range.
        let italic = r
            .inline_runs
            .iter()
            .find(|(_, s)| s.italic && !s.bold)
            .unwrap();
        let bold = r.inline_runs.iter().find(|(_, s)| s.bold).unwrap();
        assert!(italic.0.start <= bold.0.start);
        assert!(italic.0.end >= bold.0.end);
    }
}
