//! Shared segment action and visible-run coalescing helpers.
//!
//! Kept outside `segments.rs` so feature-specific segment providers can
//! grow without pushing the dispatcher over the conventions line cap.

use std::ops::Range;

use crate::id::SourceByte;
use crate::segment::{DisplaySegment, SegmentHit};
use crate::style::SpanStyle;

#[derive(Clone, Debug)]
pub(super) enum Action {
    Hide,
    Replace {
        display: String,
        style: SpanStyle,
        hit: SegmentHit,
    },
}

#[derive(Clone, Debug)]
pub(super) struct ActionRange {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) action: Action,
}

pub(super) fn snap_to_line_char_boundary(
    line_text: &str,
    line_start: usize,
    line_end: usize,
    byte: usize,
) -> usize {
    let clamped = byte.clamp(line_start, line_end);
    let mut local = clamped - line_start;
    local = local.min(line_text.len());
    while local > 0 && !line_text.is_char_boundary(local) {
        local -= 1;
    }
    line_start + local
}

pub(super) fn dedup_overlapping_actions(actions: Vec<ActionRange>) -> Vec<ActionRange> {
    // Trim overlaps: keep the earlier-pushed (=lower priority) action's
    // non-overlapping prefix, and let the later action cover the overlap.
    let mut out: Vec<ActionRange> = Vec::with_capacity(actions.len());
    for a in actions {
        let mut trimmed: Vec<ActionRange> = Vec::new();
        for existing in out.drain(..) {
            if existing.end <= a.start || existing.start >= a.end {
                trimmed.push(existing);
                continue;
            }
            if existing.start < a.start {
                trimmed.push(ActionRange {
                    start: existing.start,
                    end: a.start,
                    action: existing.action.clone(),
                });
            }
            if existing.end > a.end {
                trimmed.push(ActionRange {
                    start: a.end,
                    end: existing.end,
                    action: existing.action.clone(),
                });
            }
        }
        trimmed.push(ActionRange {
            start: a.start,
            end: a.end,
            action: a.action.clone(),
        });
        trimmed.sort_by_key(|x| (x.start, x.end));
        out = trimmed;
    }
    out
}

pub(super) fn coalesce_segments(
    line_start: usize,
    line_end: usize,
    line_text: &str,
    block_style: SpanStyle,
    style_overlays: &[(Range<usize>, SpanStyle)],
    hit_overlays: &[(Range<usize>, SegmentHit)],
    actions: &[ActionRange],
) -> Vec<DisplaySegment> {
    let mut segments: Vec<DisplaySegment> = Vec::new();
    let mut cursor = line_start;
    let mut action_iter = actions.iter().peekable();

    while cursor < line_end {
        while let Some(a) = action_iter.peek() {
            if a.end <= cursor {
                action_iter.next();
            } else {
                break;
            }
        }
        let next_action = action_iter.peek();

        let segment_end = match next_action {
            Some(a) if a.start <= cursor => {
                let end = a.end.min(line_end);
                match &a.action {
                    Action::Hide => {
                        segments.push(DisplaySegment::Hidden {
                            source: SourceByte::from_usize(cursor)..SourceByte::from_usize(end),
                        });
                    }
                    Action::Replace {
                        display,
                        style,
                        hit,
                    } => {
                        segments.push(DisplaySegment::Replace {
                            source: SourceByte::from_usize(cursor)..SourceByte::from_usize(end),
                            display: display.clone().into_boxed_str(),
                            style: *style,
                            hit: hit.clone(),
                        });
                    }
                }
                end
            }
            Some(a) => {
                let end = a.start.min(line_end);
                push_visible_segments(
                    &mut segments,
                    cursor,
                    end,
                    line_start,
                    line_text,
                    block_style,
                    style_overlays,
                    hit_overlays,
                );
                end
            }
            None => {
                push_visible_segments(
                    &mut segments,
                    cursor,
                    line_end,
                    line_start,
                    line_text,
                    block_style,
                    style_overlays,
                    hit_overlays,
                );
                line_end
            }
        };
        if segment_end <= cursor {
            break;
        }
        cursor = segment_end;
    }

    if segments.is_empty() {
        segments.push(DisplaySegment::Visible {
            source: SourceByte::from_usize(line_start)..SourceByte::from_usize(line_end),
            style: block_style,
            hit: SegmentHit::None,
        });
    }
    segments
}

#[allow(clippy::too_many_arguments)]
fn push_visible_segments(
    out: &mut Vec<DisplaySegment>,
    start: usize,
    end: usize,
    line_start: usize,
    line_text: &str,
    block_style: SpanStyle,
    style_overlays: &[(Range<usize>, SpanStyle)],
    hit_overlays: &[(Range<usize>, SegmentHit)],
) {
    if start >= end {
        return;
    }
    let mut boundaries: Vec<usize> = vec![start, end];
    for (r, _) in style_overlays {
        if r.start > start && r.start < end {
            boundaries.push(r.start);
        }
        if r.end > start && r.end < end {
            boundaries.push(r.end);
        }
    }
    for (r, _) in hit_overlays {
        if r.start > start && r.start < end {
            boundaries.push(r.start);
        }
        if r.end > start && r.end < end {
            boundaries.push(r.end);
        }
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    for win in boundaries.windows(2) {
        let s = win[0];
        let e = win[1];
        if s >= e {
            continue;
        }
        let mid = (s + e) / 2;
        let mut style = block_style;
        for (r, st) in style_overlays {
            if r.start <= mid && mid < r.end {
                style = style.merge(*st);
            }
        }
        let mut hit = SegmentHit::None;
        for (r, candidate) in hit_overlays {
            if r.start <= mid && mid < r.end {
                hit = candidate.clone();
            }
        }
        if s - line_start >= line_text.len() {
            out.push(DisplaySegment::Visible {
                source: SourceByte::from_usize(s)..SourceByte::from_usize(e),
                style,
                hit,
            });
            continue;
        }
        out.push(DisplaySegment::Visible {
            source: SourceByte::from_usize(s)..SourceByte::from_usize(e),
            style,
            hit,
        });
    }
}
