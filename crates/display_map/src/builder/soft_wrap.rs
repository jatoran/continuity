//! Soft-wrap pass — split a single source-line [`DisplayLineSpec`] into
//! one or more continuation specs when its measured display width
//! exceeds the configured wrap column.
//!
//! Extracted from `builder.rs` to keep the parent file under the
//! 600-line conventions cap. Behavior is identical to the pre-ε.2
//! inline implementation: same width sum, same word-break preference,
//! same segment slicing.

use unicode_segmentation::UnicodeSegmentation;

use crate::error::Error;
use crate::id::{DisplayByte, SourceByte};
use crate::line::DisplayLineSpec;
use crate::segment::DisplaySegment;
use crate::style::SpanStyle;
use crate::wrap::{continuation_wrap_budget_dip, hanging_indent_dip, WidthMeasure, WrapConfig};

/// If the line fits, return `[spec]`; otherwise split it at word-aware
/// break points (falling back to grapheme breaks when no whitespace is
/// available) and return the continuation specs.
pub(super) fn soft_wrap_spec(
    spec: DisplayLineSpec,
    line_text: &str,
    wrap: WrapConfig,
    measure: &mut dyn WidthMeasure,
) -> Result<Vec<DisplayLineSpec>, Error> {
    let max_width = wrap.width_dip as f32;
    // Sum per-segment scaled widths so heading runs (font_scale > 1)
    // and superscript runs (font_scale < 1) get measured at the size
    // they actually render at. Without this, a heading line tallies
    // as body width and overflows the painted right edge by ~40 %.
    let mut total_width = 0.0_f32;
    for seg in &spec.segments {
        let bytes = seg.display_bytes(line_text, spec.source_byte_start);
        if bytes.is_empty() {
            continue;
        }
        let style = seg.style().copied().unwrap_or_else(SpanStyle::body);
        let w = measure.measure(bytes, &style);
        if !w.is_finite() || w < 0.0 {
            return Err(Error::BadMeasurement(w));
        }
        total_width += w;
    }
    if total_width <= max_width {
        return Ok(vec![spec]);
    }

    // Continuation rows are painted shifted right by the line's hanging
    // indent (leading whitespace + list marker), so they get a reduced
    // budget — otherwise their painted right edge overflows the text
    // column by exactly the indent width.
    let hang_indent = hanging_indent_dip(line_text, measure);
    let continuation_max_width = continuation_wrap_budget_dip(max_width, hang_indent);

    // Find wrap breakpoints in display space, then translate back to
    // source bytes via the spec's `display_to_source` table.
    let break_points = grapheme_word_break_points_styled(
        &spec,
        line_text,
        max_width,
        continuation_max_width,
        measure,
    );
    if break_points.is_empty() {
        return Ok(vec![spec]);
    }

    let source_breaks: Vec<usize> = break_points
        .iter()
        .filter_map(|&db| {
            spec.display_to_source(DisplayByte::from_usize(db))
                .map(|s| s.as_usize())
        })
        .collect();

    // Build N sub-specs by splitting segments at each source break.
    let line_start = spec.source_byte_start.as_usize();
    let line_end = spec.source_byte_end.as_usize();
    let mut starts = vec![line_start];
    starts.extend_from_slice(&source_breaks);
    starts.push(line_end);
    starts.sort_unstable();
    starts.dedup();

    let segments = spec.segments.clone();
    let source_line = spec.source_line;
    let mut out = Vec::with_capacity(starts.len() - 1);
    for win in starts.windows(2) {
        let s = win[0];
        let e = win[1];
        if s >= e {
            continue;
        }
        let sub_segments = slice_segments(&segments, s, e);
        let is_cont = s != line_start;
        let sub_text = if s >= line_start && e <= line_start + line_text.len() {
            &line_text[s - line_start..e - line_start]
        } else {
            line_text
        };
        out.push(DisplayLineSpec::new(
            source_line,
            SourceByte::from_usize(s),
            SourceByte::from_usize(e),
            is_cont,
            sub_segments,
            sub_text,
        ));
    }
    Ok(out)
}

fn slice_segments(segments: &[DisplaySegment], start: usize, end: usize) -> Vec<DisplaySegment> {
    let mut out = Vec::new();
    for seg in segments {
        let r = seg.source_range();
        let r_s = r.start.as_usize();
        let r_e = r.end.as_usize();
        if r_e <= start || r_s >= end {
            continue;
        }
        let s = r_s.max(start);
        let e = r_e.min(end);
        if s >= e {
            continue;
        }
        let trimmed = trim_segment(seg, s, e);
        out.push(trimmed);
    }
    out
}

fn trim_segment(seg: &DisplaySegment, start: usize, end: usize) -> DisplaySegment {
    match seg {
        DisplaySegment::Visible { style, hit, .. } => DisplaySegment::Visible {
            source: SourceByte::from_usize(start)..SourceByte::from_usize(end),
            style: *style,
            hit: hit.clone(),
        },
        DisplaySegment::Hidden { .. } => DisplaySegment::Hidden {
            source: SourceByte::from_usize(start)..SourceByte::from_usize(end),
        },
        DisplaySegment::Replace {
            display,
            style,
            hit,
            ..
        } => {
            // A Replace is atomic: keep its full display text on the
            // sub-spec covering its source range. If the slice cuts
            // the source mid-replace, attach it to whichever sub-spec
            // contains the start of the replace.
            DisplaySegment::Replace {
                source: SourceByte::from_usize(start)..SourceByte::from_usize(end),
                display: display.clone(),
                style: *style,
                hit: hit.clone(),
            }
        }
    }
}

/// Walk the spec's segments and find display-byte offsets where the
/// running width exceeds the wrap column. Prefer the most-recent
/// whitespace boundary for word-aware wrapping; fall back to the
/// grapheme boundary otherwise. Each segment's graphemes are measured
/// under that segment's own [`SpanStyle`] so heading runs (1.42× body)
/// and superscript runs (0.70× body) wrap at the right column.
///
/// The first row is budgeted at `max_width`; every later row at
/// `continuation_max_width` (the wrap column minus the line's hanging
/// indent), matching where the painter actually places continuation
/// rows.
fn grapheme_word_break_points_styled(
    spec: &DisplayLineSpec,
    line_text: &str,
    max_width: f32,
    continuation_max_width: f32,
    measure: &mut dyn WidthMeasure,
) -> Vec<usize> {
    let mut breaks: Vec<usize> = Vec::new();
    let mut line_start_byte = 0_usize;
    let mut last_word_break: Option<usize> = None;
    // `running` is the row-relative width accumulated since the last break.
    // `running_at_word_break` captures `running` (including the trailing
    // whitespace) at `last_word_break`, so the carry-over width after a
    // word-boundary break is exact and segment-independent:
    // `(running + w) - running_at_word_break`. The previous code re-measured
    // a *current-segment-only* suffix, which dropped the width of any styled
    // segment (inline code, bold, link) the carried word spanned and
    // over-filled the continuation row past the column — the soft-wrap
    // overflow on inline-code / styled lines.
    let mut running = 0.0_f32;
    let mut running_at_word_break = 0.0_f32;
    let mut segment_base = 0_usize;
    let mut row_budget = max_width;

    for seg in &spec.segments {
        let bytes = seg.display_bytes(line_text, spec.source_byte_start);
        if bytes.is_empty() {
            continue;
        }
        let style = seg.style().copied().unwrap_or_else(SpanStyle::body);
        for (rel_off, g) in bytes.grapheme_indices(true) {
            let byte_off = segment_base + rel_off;
            let w = measure.measure(g, &style);
            if g.chars().any(|c| c.is_whitespace()) {
                last_word_break = Some(byte_off + g.len());
                running_at_word_break = running + w;
            }
            if running + w > row_budget && byte_off > line_start_byte {
                let word_break = last_word_break.filter(|c| *c > line_start_byte);
                let cut = word_break.unwrap_or(byte_off);
                breaks.push(cut);
                line_start_byte = cut;
                row_budget = continuation_max_width;
                // Carry-over width with no re-measure: a word-boundary break
                // carries everything after the break point; a hard grapheme
                // break (no usable word break) starts the new row at the
                // current grapheme, whose width is `w`.
                running = match word_break {
                    Some(_) => (running + w - running_at_word_break).max(0.0),
                    None => w,
                };
                last_word_break = None;
                running_at_word_break = 0.0;
            } else {
                running += w;
            }
        }
        segment_base += bytes.len();
    }
    breaks
}
