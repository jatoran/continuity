//! Byte-table builders for [`crate::line::DisplayLineSpec`].
//!
//! Pure helpers extracted from `line.rs` to keep the parent file under
//! the 600-line conventions cap. None of these escape the
//! `crate::line` module; each is called exactly once from
//! `DisplayLineSpec::new` to bake one of the four lookup tables that
//! back source↔display mapping at paint time:
//!
//! - `build_display_text` — concatenates segment display strings.
//! - `build_source_to_display` — `source byte → display byte` lookup.
//! - `build_display_to_source` — inverse lookup (absolute source bytes).
//! - `build_display_byte_to_utf16` — display byte → utf-16 prefix.
//! - `compute_content_stamp` — stable hash for the layout cache.
//!
//! Plus one debug-assert helper:
//! - `assert_sorted_and_contiguous` — segment-list invariant check.

use std::hash::{Hash, Hasher};

use crate::id::SourceByte;
use crate::segment::DisplaySegment;

pub(super) fn assert_sorted_and_contiguous(
    segments: &[DisplaySegment],
    line_start: SourceByte,
    line_end: SourceByte,
) {
    let mut cursor = line_start.raw();
    for seg in segments {
        let r = seg.source_range();
        assert!(
            r.start.raw() == cursor,
            "segments must be contiguous from line start; cursor={} segment={:?}",
            cursor,
            r,
        );
        assert!(r.end.raw() >= r.start.raw(), "segment range out of order");
        cursor = r.end.raw();
    }
    assert!(
        cursor == line_end.raw(),
        "segments must cover the line ({}..{}, got cursor {})",
        line_start.raw(),
        line_end.raw(),
        cursor,
    );
}

pub(super) fn build_display_text(
    segments: &[DisplaySegment],
    source_line_text: &str,
    line_start: SourceByte,
) -> String {
    let mut s = String::new();
    for seg in segments {
        s.push_str(seg.display_bytes(source_line_text, line_start));
    }
    s
}

pub(super) fn build_source_to_display(
    segments: &[DisplaySegment],
    line_start: SourceByte,
    line_end: SourceByte,
    source_line_text: &str,
) -> Vec<u32> {
    let span_len = (line_end.raw() - line_start.raw()) as usize;
    let mut table = vec![0_u32; span_len + 1];
    let mut display_cursor: u32 = 0;
    let mut last_known_display: u32 = 0;
    for seg in segments {
        let src = seg.source_range();
        let src_start_local = (src.start.raw() - line_start.raw()) as usize;
        let src_end_local = (src.end.raw() - line_start.raw()) as usize;
        match seg {
            DisplaySegment::Visible { .. } => {
                let line_len = source_line_text.len();
                let count = src_end_local.saturating_sub(src_start_local);
                for i in 0..=count {
                    let idx = src_start_local + i;
                    if idx < table.len() {
                        let mapped = display_cursor + i as u32;
                        // Clamp to display length when the source line is
                        // shorter than expected (defensive).
                        let _ = line_len;
                        table[idx] = mapped;
                        last_known_display = mapped;
                    }
                }
                display_cursor += count as u32;
            }
            DisplaySegment::Hidden { .. } => {
                for i in 0..=(src_end_local - src_start_local) {
                    let idx = src_start_local + i;
                    if idx < table.len() {
                        table[idx] = u32::MAX;
                    }
                }
            }
            DisplaySegment::Replace { display, .. } => {
                let span_len = src_end_local - src_start_local;
                let display_len = display.len() as u32;
                if src_start_local < table.len() {
                    table[src_start_local] = display_cursor;
                }
                if src_start_local + span_len < table.len() {
                    table[src_start_local + span_len] = display_cursor + display_len;
                }
                for i in 1..span_len {
                    let idx = src_start_local + i;
                    if idx < table.len() {
                        table[idx] = u32::MAX;
                    }
                }
                display_cursor += display_len;
                last_known_display = display_cursor;
            }
        }
    }
    // Ensure the final entry is the total display length (in case the
    // last segment was Hidden / Replace and we left u32::MAX trailing).
    if let Some(last) = table.last_mut() {
        if *last == u32::MAX {
            *last = last_known_display;
        }
    }
    table
}

pub(super) fn build_display_to_source(
    segments: &[DisplaySegment],
    line_start: SourceByte,
    source_line_text: &str,
) -> Vec<u32> {
    let total_display: usize = segments
        .iter()
        .map(|s| match s {
            DisplaySegment::Visible { source, .. } => {
                (source.end.raw() - source.start.raw()) as usize
            }
            DisplaySegment::Hidden { .. } => 0,
            DisplaySegment::Replace { display, .. } => display.len(),
        })
        .sum();
    let mut table = Vec::with_capacity(total_display + 1);
    let source_cursor = line_start.raw();
    let _ = source_cursor;
    let mut last_visible_source = line_start.raw();
    for seg in segments {
        match seg {
            DisplaySegment::Visible { source, .. } => {
                push_visible_display_to_source(&mut table, source_line_text, line_start, source);
                last_visible_source = source.end.raw();
            }
            DisplaySegment::Hidden { source } => {
                // Hidden segments contribute no display bytes; advance the
                // source cursor we use for replacements past them.
                last_visible_source = source.end.raw();
            }
            DisplaySegment::Replace {
                source, display, ..
            } => {
                // Every byte of the replacement maps back to the *start*
                // of the source range. End-of-replacement maps to end.
                for _ in 0..display.len() {
                    table.push(source.start.raw());
                }
                last_visible_source = source.end.raw();
            }
        }
    }
    table.push(last_visible_source);
    table
}

fn push_visible_display_to_source(
    table: &mut Vec<u32>,
    source_line_text: &str,
    line_start: SourceByte,
    source: &std::ops::Range<SourceByte>,
) {
    let line_start_raw = line_start.raw() as usize;
    let source_start = source.start.raw() as usize;
    let source_end = source.end.raw() as usize;
    let local_start = source_start
        .saturating_sub(line_start_raw)
        .min(source_line_text.len());
    let local_end = source_end
        .saturating_sub(line_start_raw)
        .min(source_line_text.len());
    let Some(text) = source_line_text.get(local_start..local_end) else {
        return;
    };
    let mut local_byte = local_start;
    for ch in text.chars() {
        let source_char_start = line_start_raw + local_byte;
        for _ in 0..ch.len_utf8() {
            table.push(source_char_start as u32);
        }
        local_byte += ch.len_utf8();
    }
}

pub(super) fn build_display_byte_to_utf16(display_text: &str) -> Vec<u32> {
    let mut table = Vec::with_capacity(display_text.len() + 1);
    table.push(0);
    let mut utf16: u32 = 0;
    for ch in display_text.chars() {
        let bytes = ch.len_utf8();
        let units = ch.len_utf16() as u32;
        for _ in 0..bytes {
            table.push(utf16 + units);
        }
        utf16 += units;
        // The intermediate bytes of a multi-byte char all map to the
        // post-char utf16 (DirectWrite treats partial-char positions as
        // the next code unit).
    }
    // Final entry is the post-text utf16 count.
    if let Some(last) = table.last_mut() {
        *last = utf16;
    }
    table
}

pub(super) fn compute_content_stamp(display_text: &str, segments: &[DisplaySegment]) -> u64 {
    let mut h = ahash::AHasher::default();
    display_text.hash(&mut h);
    for seg in segments {
        match seg {
            DisplaySegment::Visible { style, .. } | DisplaySegment::Replace { style, .. } => {
                style.bold.hash(&mut h);
                style.italic.hash(&mut h);
                style.strikethrough.hash(&mut h);
                style.underline.hash(&mut h);
                style.font_scale.hash(&mut h);
                std::mem::discriminant(&style.role).hash(&mut h);
                match style.role {
                    crate::style::SpanRole::Heading(level) => level.hash(&mut h),
                    crate::style::SpanRole::Syntax(kind) => kind.hash(&mut h),
                    _ => {}
                }
            }
            DisplaySegment::Hidden { .. } => {}
        }
    }
    h.finish()
}
