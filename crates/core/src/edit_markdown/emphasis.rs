//! Caret-inside-span detection for emphasis toggles.
//!
//! When `Ctrl+B` / `Ctrl+I` / strikethrough / inline-code is pressed with a
//! bare caret (no selection) that sits *inside* an existing emphasis span,
//! the toggle should strip the span rather than nest a fresh empty pair.
//! [`enclosing_delimiter_runs`] locates the opening and closing delimiter
//! runs that bracket the caret on its own line.
//!
//! The runs are matched as literal delimiter strings (`**`, `*`, `~~`,
//! `` ` ``). Single-asterisk (italic) matching skips any `*` that is part of
//! a `**` run so a caret inside `**bold**` is recognised by the bold pass
//! (double-asterisk) and not mis-stripped by the italic pass — the caller
//! checks bold before italic for exactly this reason.

use ropey::Rope;

use crate::edit_planning::line_content_end;
use crate::Error;

/// Absolute byte range for one emphasis delimiter run.
pub(crate) type DelimiterRun = (usize, usize);
/// Opening and closing delimiter runs that enclose a caret.
pub(crate) type EnclosingDelimiterRuns = (DelimiterRun, DelimiterRun);

/// Absolute byte ranges `(start, end)` of the opening and closing delimiter
/// runs enclosing `caret_byte`, when the caret sits between a matched pair of
/// `open`/`close` delimiters on its own line. Returns `Ok(None)` when no such
/// enclosing pair exists.
///
/// Only `open` is inspected for run discovery — emphasis delimiters are
/// symmetric (`open == close`) for every [`crate::EmphasisKind`], so a single
/// scan suffices. `close` is accepted for symmetry with the call site and to
/// keep the helper honest if an asymmetric delimiter is ever added.
pub(crate) fn enclosing_delimiter_runs(
    rope: &Rope,
    caret_byte: usize,
    open: &str,
    close: &str,
) -> Result<Option<EnclosingDelimiterRuns>, Error> {
    debug_assert_eq!(open, close, "emphasis delimiters are symmetric");
    let line = rope.byte_to_line(caret_byte);
    let line_start = rope.line_to_byte(line);
    let line_end = line_content_end(rope, line);
    if caret_byte < line_start || caret_byte > line_end {
        return Ok(None);
    }
    let line_text = rope.byte_slice(line_start..line_end).to_string();
    let caret_in_line = caret_byte - line_start;

    let runs = delimiter_runs(&line_text, open);
    // Pair consecutive runs: (0,1), (2,3), … Each pair brackets one span.
    let mut iter = runs.chunks_exact(2);
    for pair in iter.by_ref() {
        let open_run = pair[0];
        let close_run = pair[1];
        let inner_start = open_run.1; // end of opening delimiter
        let inner_end = close_run.0; // start of closing delimiter
                                     // Caret must sit between the inner edges (inclusive) to count as
                                     // "inside" — including resting directly on either inner edge.
        if caret_in_line >= inner_start && caret_in_line <= inner_end {
            return Ok(Some((
                (line_start + open_run.0, line_start + open_run.1),
                (line_start + close_run.0, line_start + close_run.1),
            )));
        }
    }
    Ok(None)
}

/// Line-relative `(start, end)` byte ranges of every delimiter run matching
/// `delim`, left to right, non-overlapping. For the single-asterisk italic
/// delimiter, `*` characters that are part of a `**` run are skipped so the
/// italic pass never matches bold markers.
fn delimiter_runs(line: &str, delim: &str) -> Vec<(usize, usize)> {
    let bytes = line.as_bytes();
    let dlen = delim.len();
    let dbytes = delim.as_bytes();
    let is_single_asterisk = delim == "*";
    let mut runs = Vec::new();
    let mut i = 0;
    while i + dlen <= bytes.len() {
        if &bytes[i..i + dlen] == dbytes {
            if is_single_asterisk {
                // Skip a `*` that is part of a `**` (bold) run: look at the
                // neighbours. A `*` preceded or followed by another `*` is
                // bold territory.
                let prev_star = i > 0 && bytes[i - 1] == b'*';
                let next_star = i + 1 < bytes.len() && bytes[i + 1] == b'*';
                if prev_star || next_star {
                    i += 1;
                    continue;
                }
            }
            runs.push((i, i + dlen));
            i += dlen;
        } else {
            i += 1;
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(line: &str, delim: &str) -> Vec<(usize, usize)> {
        delimiter_runs(line, delim)
    }

    #[test]
    fn bold_runs_are_double_asterisk_pairs() {
        assert_eq!(runs("**a**", "**"), vec![(0, 2), (3, 5)]);
    }

    #[test]
    fn italic_runs_skip_bold_markers() {
        // No single-asterisk runs inside a pure bold span.
        assert_eq!(runs("**a**", "*"), Vec::<(usize, usize)>::new());
        // Genuine italic.
        assert_eq!(runs("*a*", "*"), vec![(0, 1), (2, 3)]);
    }

    #[test]
    fn strikethrough_runs_are_double_tilde() {
        assert_eq!(runs("~~x~~", "~~"), vec![(0, 2), (3, 5)]);
    }

    #[test]
    fn inline_code_runs_are_single_backtick() {
        assert_eq!(runs("`c`", "`"), vec![(0, 1), (2, 3)]);
    }

    fn buf(line: &str) -> Rope {
        Rope::from_str(line)
    }

    #[test]
    fn finds_enclosing_bold_for_caret_inside() {
        let rope = buf("**bold**");
        // Caret at byte 4 (inside "bold").
        let got = enclosing_delimiter_runs(&rope, 4, "**", "**")
            .expect("ok")
            .expect("some");
        assert_eq!(got, ((0, 2), (6, 8)));
    }

    #[test]
    fn finds_enclosing_bold_for_caret_at_inner_edge() {
        let rope = buf("**bold**");
        // Caret at byte 2 — directly after the opening `**`.
        let got = enclosing_delimiter_runs(&rope, 2, "**", "**")
            .expect("ok")
            .expect("some");
        assert_eq!(got, ((0, 2), (6, 8)));
    }

    #[test]
    fn no_enclosing_span_in_plain_text() {
        let rope = buf("plain text");
        assert!(enclosing_delimiter_runs(&rope, 3, "**", "**")
            .expect("ok")
            .is_none());
    }

    #[test]
    fn italic_caret_inside_bold_finds_nothing() {
        let rope = buf("**bold**");
        // Italic pass over a bold span must not match.
        assert!(enclosing_delimiter_runs(&rope, 4, "*", "*")
            .expect("ok")
            .is_none());
    }
}
