//! Pure helpers for shifting cached segment lists onto a new source-byte
//! origin. Lives next to [`crate::segment_cache`] so the bucket store
//! stays under the 600-line cap.

use std::ops::Range;

use crate::id::SourceByte;
use crate::segment::{DisplaySegment, SegmentHit};

pub(super) fn shift_segment(segment: &DisplaySegment, delta: i64) -> DisplaySegment {
    match segment {
        DisplaySegment::Visible { source, style, hit } => DisplaySegment::Visible {
            source: shift_range(source, delta),
            style: *style,
            hit: shift_hit(hit, delta),
        },
        DisplaySegment::Hidden { source } => DisplaySegment::Hidden {
            source: shift_range(source, delta),
        },
        DisplaySegment::Replace {
            source,
            display,
            style,
            hit,
        } => DisplaySegment::Replace {
            source: shift_range(source, delta),
            display: display.clone(),
            style: *style,
            hit: shift_hit(hit, delta),
        },
    }
}

fn shift_hit(hit: &SegmentHit, delta: i64) -> SegmentHit {
    match hit {
        SegmentHit::None => SegmentHit::None,
        SegmentHit::Checkbox { toggle, checked } => SegmentHit::Checkbox {
            toggle: shift_source_byte(*toggle, delta),
            checked: *checked,
        },
        SegmentHit::Link { url } => SegmentHit::Link {
            url: shift_range(url, delta),
        },
        SegmentHit::FootnoteReference { label, definition } => SegmentHit::FootnoteReference {
            label: label.clone(),
            definition: definition.as_ref().map(|range| shift_range(range, delta)),
        },
        SegmentHit::FootnoteDefinition {
            label,
            first_reference,
        } => SegmentHit::FootnoteDefinition {
            label: label.clone(),
            first_reference: first_reference
                .as_ref()
                .map(|range| shift_range(range, delta)),
        },
    }
}

fn shift_range(range: &Range<SourceByte>, delta: i64) -> Range<SourceByte> {
    shift_source_byte(range.start, delta)..shift_source_byte(range.end, delta)
}

fn shift_source_byte(byte: SourceByte, delta: i64) -> SourceByte {
    let shifted = i64::from(byte.raw()).saturating_add(delta).max(0);
    SourceByte(u32::try_from(shifted).unwrap_or(u32::MAX))
}

pub(super) fn estimate_segments_bytes(segments: &[DisplaySegment]) -> usize {
    let base = std::mem::size_of_val(segments);
    segments.iter().fold(base, |total, segment| match segment {
        DisplaySegment::Visible { hit, .. } => total.saturating_add(estimate_hit_bytes(hit)),
        DisplaySegment::Hidden { .. } => total,
        DisplaySegment::Replace { display, hit, .. } => total
            .saturating_add(display.len())
            .saturating_add(estimate_hit_bytes(hit)),
    })
}

fn estimate_hit_bytes(hit: &SegmentHit) -> usize {
    match hit {
        SegmentHit::FootnoteReference { label, .. }
        | SegmentHit::FootnoteDefinition { label, .. } => label.len(),
        SegmentHit::None | SegmentHit::Checkbox { .. } | SegmentHit::Link { .. } => 0,
    }
}
