//! Per-segment formatting helpers — turns one
//! [`continuity_config::StatusBarSegment`] enum value into one
//! [`continuity_render::StatusBarSegmentDraw`], plus the selection /
//! numeric-sum / position formatters they delegate to.
//!
//! Suppression of a segment (e.g. `Selection` with a caret-only
//! selection, `NumericSum` with no parsable numbers, `IdleStale` while
//! input is recent) is signalled by returning `None`.
//!
//! Thread ownership: UI thread of one window.

use continuity_buffer::FileAssociation;
use continuity_config::StatusBarSegment;
use continuity_render::{StatusBarSegmentDraw, StatusBarSegmentKind};
use continuity_text::Selection;
use ropey::Rope;

use crate::window_status_bar::rope_counts::RopeStatusCounts;
use crate::window_status_bar_idle::format_idle_stale;
use crate::window_view_options::StatusCountMode;

/// Format one segment, or return `None` when the segment is suppressed
/// (e.g. selection stats with no active selection).
#[allow(clippy::too_many_arguments)]
pub(super) fn build_segment(
    kind: StatusBarSegment,
    rope: &Rope,
    selections: &[Selection],
    file: Option<&FileAssociation>,
    count_mode: StatusCountMode,
    idle_ms: u64,
    counts: &RopeStatusCounts,
) -> Option<StatusBarSegmentDraw> {
    match kind {
        StatusBarSegment::Position => Some(StatusBarSegmentDraw {
            text: format_position(selections),
            kind: StatusBarSegmentKind::Position,
            hover: Some("Go to line".into()),
            alpha: 1.0,
        }),
        StatusBarSegment::Chars => Some(StatusBarSegmentDraw {
            text: format_count_cached(count_mode, counts),
            kind: StatusBarSegmentKind::Chars,
            hover: Some("Click to cycle char / word / line / byte".into()),
            alpha: 1.0,
        }),
        StatusBarSegment::Words => Some(StatusBarSegmentDraw {
            text: format!("{} words", counts.word_count),
            kind: StatusBarSegmentKind::Words,
            hover: None,
            alpha: 1.0,
        }),
        StatusBarSegment::Lines => Some(StatusBarSegmentDraw {
            text: format!("{} / {} lines", counts.non_empty_lines, counts.total_lines),
            kind: StatusBarSegmentKind::Lines,
            hover: None,
            alpha: 1.0,
        }),
        StatusBarSegment::Selection => {
            format_selection(rope, selections).map(|text| StatusBarSegmentDraw {
                text,
                kind: StatusBarSegmentKind::Selection,
                hover: None,
                alpha: 1.0,
            })
        }
        StatusBarSegment::NumericSum => {
            format_numeric_sum(rope, selections).map(|text| StatusBarSegmentDraw {
                text,
                kind: StatusBarSegmentKind::NumericSum,
                hover: None,
                alpha: 1.0,
            })
        }
        StatusBarSegment::Encoding => Some(StatusBarSegmentDraw {
            text: encoding_label(file).to_string(),
            kind: StatusBarSegmentKind::Encoding,
            hover: Some("Reload with encoding…".into()),
            alpha: 1.0,
        }),
        StatusBarSegment::LineEndings => Some(StatusBarSegmentDraw {
            text: counts.line_ending.label().into(),
            kind: StatusBarSegmentKind::LineEndings,
            hover: Some("Toggle line endings (LF ↔ CRLF)".into()),
            alpha: 1.0,
        }),
        StatusBarSegment::Language => Some(StatusBarSegmentDraw {
            text: language_label(file).to_string(),
            kind: StatusBarSegmentKind::Language,
            hover: None,
            alpha: 1.0,
        }),
        StatusBarSegment::IdleStale => {
            format_idle_stale(idle_ms).map(|text| StatusBarSegmentDraw {
                text,
                kind: StatusBarSegmentKind::IdleStale,
                hover: None,
                alpha: 1.0,
            })
        }
    }
}

/// Cached variant of [`format_count`]. Reads from
/// [`RopeStatusCounts`] instead of walking the rope.
fn format_count_cached(mode: StatusCountMode, counts: &RopeStatusCounts) -> String {
    match mode {
        StatusCountMode::Chars => format!("{} chars", counts.char_count),
        StatusCountMode::Words => format!("{} words", counts.word_count),
        StatusCountMode::Lines => {
            format!("{} / {} lines", counts.non_empty_lines, counts.total_lines)
        }
        StatusCountMode::Bytes => format!("{} bytes", counts.byte_count),
    }
}

pub(super) fn format_position(selections: &[Selection]) -> String {
    let (line, col) = selections
        .first()
        .map_or((0u32, 0u32), |s| (s.head.line, s.head.byte_in_line));
    format!("Ln {}, Col {}", line + 1, col + 1)
}

#[cfg(test)]
pub(super) fn format_count(rope: &Rope, mode: StatusCountMode) -> String {
    match mode {
        StatusCountMode::Chars => format!("{} chars", rope.len_chars()),
        StatusCountMode::Words => format!("{} words", count_words(rope)),
        StatusCountMode::Lines => format_lines(rope),
        StatusCountMode::Bytes => format!("{} bytes", rope.len_bytes()),
    }
}

#[cfg(test)]
pub(super) fn count_words(rope: &Rope) -> usize {
    // Simple whitespace-split count over rope chunks. Good enough for
    // a status-bar counter — Phase F4 will revisit if tables introduce
    // mixed-script word boundaries.
    let mut total = 0usize;
    let mut in_word = false;
    for chunk in rope.chunks() {
        for c in chunk.chars() {
            if c.is_whitespace() {
                in_word = false;
            } else if !in_word {
                in_word = true;
                total += 1;
            }
        }
    }
    total
}

#[cfg(test)]
pub(super) fn format_lines(rope: &Rope) -> String {
    let total = rope.len_lines();
    let mut non_empty = 0usize;
    for i in 0..total {
        let line = rope.line(i);
        if line.chars().any(|c| !c.is_whitespace()) {
            non_empty += 1;
        }
    }
    format!("{non_empty} / {total} lines")
}

pub(super) fn format_selection(rope: &Rope, selections: &[Selection]) -> Option<String> {
    let sel = selections.first()?;
    if sel.anchor == sel.head {
        return None;
    }
    let text = selection_text(rope, sel);
    let chars = text.chars().count();
    let words = count_words_in_str(&text);
    let lines = if text.is_empty() {
        0
    } else {
        text.matches('\n').count() + 1
    };
    Some(format!("Sel {chars}c · {words}w · {lines}l"))
}

pub(super) fn format_numeric_sum(rope: &Rope, selections: &[Selection]) -> Option<String> {
    let sel = selections.first()?;
    if sel.anchor == sel.head {
        return None;
    }
    let text = selection_text(rope, sel);
    let mut sum: f64 = 0.0;
    let mut had_token = false;
    for tok in text.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+')) {
        if tok.is_empty() {
            continue;
        }
        if let Ok(v) = tok.parse::<f64>() {
            sum += v;
            had_token = true;
        }
    }
    if !had_token {
        return None;
    }
    // Format compactly: integers without a trailing `.0`, floats with up
    // to 6 significant digits.
    if sum.fract() == 0.0 && sum.abs() < 1e15 {
        Some(format!("Σ {}", sum as i64))
    } else {
        Some(format!("Σ {sum:.6}"))
    }
}

fn count_words_in_str(text: &str) -> usize {
    text.split_whitespace().count()
}

fn selection_text(rope: &Rope, sel: &Selection) -> String {
    let (a, b) = (sel.anchor, sel.head);
    let (start, end) = if (a.line, a.byte_in_line) <= (b.line, b.byte_in_line) {
        (a, b)
    } else {
        (b, a)
    };
    let start_byte = position_to_byte(rope, start.line, start.byte_in_line);
    let end_byte = position_to_byte(rope, end.line, end.byte_in_line);
    if end_byte <= start_byte || end_byte > rope.len_bytes() {
        return String::new();
    }
    rope.byte_slice(start_byte..end_byte).to_string()
}

fn position_to_byte(rope: &Rope, line: u32, byte_in_line: u32) -> usize {
    let line = line as usize;
    if line >= rope.len_lines() {
        return rope.len_bytes();
    }
    let line_start = rope.line_to_byte(line);
    let line_end = if line + 1 < rope.len_lines() {
        rope.line_to_byte(line + 1)
    } else {
        rope.len_bytes()
    };
    (line_start + byte_in_line as usize).min(line_end)
}

fn encoding_label(_file: Option<&FileAssociation>) -> &'static str {
    // Continuity always normalises imported text to UTF-8 on the rope.
    // File-association metadata doesn't currently track the source
    // encoding — when reload-with-encoding lands (C2) this will return
    // the picker's last selection.
    "UTF-8"
}

fn language_label(_file: Option<&FileAssociation>) -> &'static str {
    // Phase 12 only knows two languages, and tab title heuristics drive
    // the markdown choice. Until the language field migrates onto the
    // buffer, fall back to file extension.
    match _file {
        Some(f) => {
            let ext = f
                .path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            match ext.as_str() {
                "md" | "markdown" => "markdown",
                _ => "plain",
            }
        }
        None => "plain",
    }
}
