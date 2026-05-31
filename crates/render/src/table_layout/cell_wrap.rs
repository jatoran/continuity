//! Phase F — split a pipe-table cell's content into the visual lines
//! the painter stacks.
//!
//! Two stages:
//!
//! 1. [`split_cell_on_br`] cuts the raw cell text on `<br>` / `<br/>` /
//!    `<br />` (any case) hard breaks.
//! 2. [`wrap_cell_lines`] greedy-word-wraps each `<br>`-segment to the
//!    column's inner width so over-wide content flows onto additional
//!    visual rows instead of stretching the column off the pane edge.
//!
//! The painter draws each [`CellLine`] with no further wrapping, so the
//! rendered line count equals `lines.len()` exactly — which is what the
//! display-map row reservation reserves, keeping the chrome, the
//! gutter, and the caret below a tall row aligned.
//!
//! Inline-markdown styling rides through on a line only when that line
//! is an unwrapped whole segment; a wrapped sub-line renders plain
//! (multi-line inline styling is a deliberate follow-up — wrapping the
//! style runs across break points needs per-line range re-slicing).
//!
//! Thread ownership: pure data, callable from any thread.

use std::ops::Range;

use continuity_display_map::SpanStyle;

/// One visual line of a table cell. `text` is the line's display text
/// (markers already stripped); `inline_runs` are UTF-8 byte ranges into
/// `text` carrying bold / italic / code / strike / link styling.
#[derive(Clone, Debug, PartialEq)]
pub struct CellLine {
    /// The line's display text.
    pub text: String,
    /// Per-byte style runs indexing into `text`.
    pub inline_runs: Vec<(Range<u32>, SpanStyle)>,
}

impl CellLine {
    /// A plain (unstyled) line.
    #[must_use]
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            inline_runs: Vec::new(),
        }
    }
}

/// One `<br>`-delimited segment of a cell, already inline-parsed.
pub(super) struct CellSegment {
    pub display_text: String,
    pub inline_runs: Vec<(Range<u32>, SpanStyle)>,
}

/// Split `raw` (a trimmed cell payload) on `<br>` hard breaks. Returns
/// at least one segment (the whole string when no break is present).
/// Each returned segment is trimmed of surrounding whitespace.
pub(super) fn split_cell_on_br(raw: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut rest = raw;
    loop {
        match find_br(rest) {
            Some((start, end)) => {
                segments.push(rest[..start].trim().to_string());
                rest = &rest[end..];
            }
            None => {
                segments.push(rest.trim().to_string());
                break;
            }
        }
    }
    segments
}

/// Locate the first `<br>` / `<br/>` / `<br />` (any case, any inner
/// spacing) tag in `s`. Returns the tag's byte range `[start, end)`.
fn find_br(s: &str) -> Option<(usize, usize)> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i + 3 <= bytes.len() {
        if bytes[i] == b'<'
            && matches!(bytes[i + 1], b'b' | b'B')
            && matches!(bytes[i + 2], b'r' | b'R')
        {
            let mut j = i + 3;
            while j < bytes.len() && bytes[j] == b' ' {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'/' {
                j += 1;
            }
            while j < bytes.len() && bytes[j] == b' ' {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'>' {
                return Some((i, j + 1));
            }
        }
        i += 1;
    }
    None
}

/// Flatten the cell's `<br>`-segments into the visual lines the painter
/// stacks. When `wrap_enabled`, each segment is greedy-word-wrapped to
/// `inner_width_dip` (a fitting segment keeps its inline styling; a
/// wrapped segment becomes plain sub-lines). When clipping
/// (`wrap_enabled == false`), each `<br>` segment is one line, styling
/// preserved, and the painter clips any overflow to the column edge.
/// Always returns at least one line.
pub(super) fn wrap_cell_lines(
    segments: Vec<CellSegment>,
    inner_width_dip: f32,
    wrap_enabled: bool,
    measure: &mut dyn FnMut(&str) -> f32,
) -> Vec<CellLine> {
    let mut out: Vec<CellLine> = Vec::new();
    for seg in segments {
        if wrap_enabled {
            wrap_one_segment(seg, inner_width_dip, measure, &mut out);
        } else {
            out.push(CellLine {
                text: seg.display_text,
                inline_runs: seg.inline_runs,
            });
        }
    }
    if out.is_empty() {
        out.push(CellLine::plain(String::new()));
    }
    out
}

fn wrap_one_segment(
    seg: CellSegment,
    inner_width_dip: f32,
    measure: &mut dyn FnMut(&str) -> f32,
    out: &mut Vec<CellLine>,
) {
    // Fits on one line (or no usable width to wrap into): keep the
    // whole segment and its styling.
    if inner_width_dip <= 0.0 || measure(&seg.display_text) <= inner_width_dip {
        out.push(CellLine {
            text: seg.display_text,
            inline_runs: seg.inline_runs,
        });
        return;
    }
    // Needs wrapping → greedy word break, plain sub-lines. A word that
    // is itself wider than the column is broken across lines by
    // character (otherwise a long unbroken token — a URL, a hash, or
    // typing with no spaces — would never wrap and would overflow the
    // column).
    let mut current = String::new();
    for word in seg.display_text.split(' ') {
        if current.is_empty() {
            append_word_breaking(word, inner_width_dip, measure, &mut current, out);
            continue;
        }
        let mut candidate = String::with_capacity(current.len() + 1 + word.len());
        candidate.push_str(&current);
        candidate.push(' ');
        candidate.push_str(word);
        if measure(&candidate) <= inner_width_dip {
            current = candidate;
        } else {
            out.push(CellLine::plain(std::mem::take(&mut current)));
            append_word_breaking(word, inner_width_dip, measure, &mut current, out);
        }
    }
    if !current.is_empty() {
        out.push(CellLine::plain(current));
    }
}

/// Append `word` onto `current` (which the caller guarantees is empty),
/// flushing full lines into `out` and breaking the word by character
/// whenever it grows past `inner_width_dip`. The trailing partial word
/// is left in `current` so the next word can continue the line. A single
/// char that already overflows an empty line is still placed (so a
/// pathologically narrow column makes progress instead of looping).
fn append_word_breaking(
    word: &str,
    inner_width_dip: f32,
    measure: &mut dyn FnMut(&str) -> f32,
    current: &mut String,
    out: &mut Vec<CellLine>,
) {
    for ch in word.chars() {
        let prev_len = current.len();
        current.push(ch);
        if prev_len > 0 && measure(current) > inner_width_dip {
            current.truncate(prev_len);
            out.push(CellLine::plain(std::mem::take(current)));
            current.push(ch);
        }
    }
}

/// Phase F — wrap a *caret-in-cell* cell's raw source text to
/// `inner_width_dip` while preserving every source byte. Unlike
/// [`wrap_cell_lines`] this does **not** split on `<br>` or strip
/// inline markers: the user is editing the bytes, so they must see the
/// literal source (markers, literal `<br>`), just flowed onto multiple
/// rows instead of clipped to one. The returned lines concatenate back
/// to `raw` exactly (break spaces stay at the end of the line they break
/// after), which is the contract the in-cell caret-bar painter relies on
/// to map a source-byte caret to its wrapped row. Greedy word wrap, with
/// a long unbroken token broken by character. Clipping mode
/// (`wrap_enabled == false`) keeps the whole text on one line. Always
/// returns at least one line.
pub(super) fn wrap_raw_preserving(
    raw: &str,
    inner_width_dip: f32,
    wrap_enabled: bool,
    measure: &mut dyn FnMut(&str) -> f32,
) -> Vec<CellLine> {
    if !wrap_enabled || inner_width_dip <= 0.0 || measure(raw) <= inner_width_dip {
        return vec![CellLine::plain(raw)];
    }
    let mut out: Vec<CellLine> = Vec::new();
    let mut line_start = 0usize;
    // Byte index just AFTER the last space seen on the current line —
    // the preferred cut point so the space stays on the line it ends.
    let mut last_break: Option<usize> = None;
    for (idx, ch) in raw.char_indices() {
        let candidate_end = idx + ch.len_utf8();
        if idx > line_start && measure(&raw[line_start..candidate_end]) > inner_width_dip {
            // Break at the last word boundary on this line, or — for a
            // token wider than the column — right before the overflowing
            // char so a long no-space run still makes progress.
            let cut = last_break.filter(|c| *c > line_start).unwrap_or(idx);
            out.push(CellLine::plain(raw[line_start..cut].to_string()));
            line_start = cut;
            last_break = None;
        }
        if ch == ' ' {
            last_break = Some(idx + 1);
        }
    }
    if line_start < raw.len() {
        out.push(CellLine::plain(raw[line_start..].to_string()));
    }
    if out.is_empty() {
        out.push(CellLine::plain(String::new()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str) -> CellSegment {
        CellSegment {
            display_text: text.to_string(),
            inline_runs: Vec::new(),
        }
    }

    // Monospace approximation: 10 DIP per char.
    fn measure_mono(s: &str) -> f32 {
        s.chars().count() as f32 * 10.0
    }

    #[test]
    fn no_br_yields_single_segment() {
        assert_eq!(split_cell_on_br("hello world"), vec!["hello world"]);
    }

    #[test]
    fn br_splits_and_trims() {
        assert_eq!(split_cell_on_br("first<br>second"), vec!["first", "second"]);
        assert_eq!(split_cell_on_br("a <br/> b <br /> c"), vec!["a", "b", "c"]);
        assert_eq!(split_cell_on_br("X<BR>Y"), vec!["X", "Y"]);
    }

    #[test]
    fn fits_keeps_single_line() {
        let lines = wrap_cell_lines(vec![seg("abc")], 100.0, true, &mut measure_mono);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "abc");
    }

    #[test]
    fn over_wide_wraps_to_multiple_lines() {
        // "aaa bbb ccc ddd" = each word 30 DIP; width 70 fits two words
        // ("aaa bbb" = 70) but not three.
        let lines = wrap_cell_lines(vec![seg("aaa bbb ccc ddd")], 70.0, true, &mut measure_mono);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "aaa bbb");
        assert_eq!(lines[1].text, "ccc ddd");
    }

    #[test]
    fn long_unbroken_word_char_breaks() {
        // 30 chars at 10 DIP = 300; width 100 → char-break into ~3
        // lines of ~10 chars (no spaces to break on). Reassembling the
        // lines must reproduce the original content losslessly.
        let lines = wrap_cell_lines(
            vec![seg("abcdefghijklmnopqrstuvwxyz0123")],
            100.0,
            true,
            &mut measure_mono,
        );
        assert!(
            lines.len() >= 3,
            "a long no-space word must char-break, got {} line(s)",
            lines.len()
        );
        let joined: String = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(joined, "abcdefghijklmnopqrstuvwxyz0123");
    }

    #[test]
    fn br_segments_each_wrap_independently() {
        let segs = vec![seg("aaa bbb ccc ddd"), seg("short")];
        let lines = wrap_cell_lines(segs, 70.0, true, &mut measure_mono);
        // two wrapped lines from the first segment + one for "short".
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[2].text, "short");
    }

    #[test]
    fn empty_segment_still_yields_a_line() {
        let lines = wrap_cell_lines(vec![seg("")], 100.0, true, &mut measure_mono);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "");
    }

    #[test]
    fn raw_preserving_wraps_words_and_keeps_every_byte() {
        // Width 75 fits "aaa bbb" (70) but not the following space.
        let lines = wrap_raw_preserving("aaa bbb ccc", 75.0, true, &mut measure_mono);
        assert!(lines.len() >= 2, "expected a wrap, got {}", lines.len());
        // Byte-preserving: concatenation reproduces the source exactly,
        // so the caret-bar painter can map a source byte to a line.
        let joined: String = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(joined, "aaa bbb ccc");
    }

    #[test]
    fn raw_preserving_char_breaks_long_token_losslessly() {
        let lines = wrap_raw_preserving("abcdefghijklmnop", 35.0, true, &mut measure_mono);
        assert!(lines.len() >= 4, "long token must char-break");
        let joined: String = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(joined, "abcdefghijklmnop");
    }

    #[test]
    fn raw_preserving_keeps_markers_literal() {
        // Editing view shows raw markers; wrapping must not strip them.
        let lines = wrap_raw_preserving("**bold words here**", 1000.0, true, &mut measure_mono);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "**bold words here**");
    }

    #[test]
    fn raw_preserving_clip_mode_keeps_one_line() {
        let lines = wrap_raw_preserving("aaa bbb ccc ddd eee", 50.0, false, &mut measure_mono);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "aaa bbb ccc ddd eee");
    }

    #[test]
    fn clip_mode_keeps_one_line_per_br_segment() {
        // Over-wide content does NOT wrap when clipping — it stays one
        // line per `<br>` segment and the painter clips at the edge.
        let lines = wrap_cell_lines(vec![seg("aaa bbb ccc ddd")], 70.0, false, &mut measure_mono);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "aaa bbb ccc ddd");
        // Two `<br>` segments still yield two lines even when clipping.
        let two = wrap_cell_lines(
            vec![seg("aaa bbb ccc ddd"), seg("short")],
            70.0,
            false,
            &mut measure_mono,
        );
        assert_eq!(two.len(), 2);
    }
}
