//! Segment-assembly: turn a source line + decorations + caret + folds
//! into the ordered `DisplaySegment` list that the renderer paints.
//! Extracted from `crates/display_map/src/builder.rs` to keep the
//! parent file under the 600-line conventions cap.

use continuity_decorate::{BlockKind, BlockSpan, Decorations, InlineKind, MarkerKind};
use std::ops::Range;

use super::segment_coalescing::{
    coalesce_segments, dedup_overlapping_actions, snap_to_line_char_boundary, Action, ActionRange,
};
use crate::fold::FoldRange;
use crate::id::SourceByte;
use crate::segment::{DisplaySegment, SegmentHit};
use crate::style::{SpanRole, SpanStyle};

// ---------------------------------------------------------------------------
// Reveal logic
// ---------------------------------------------------------------------------

fn block_revealed(block: &BlockSpan, caret_bytes: &[SourceByte]) -> bool {
    caret_bytes.iter().any(|c| {
        let b = c.as_usize();
        b >= block.start_byte && b <= block.end_byte
    })
}

fn line_revealed(
    decorations: &Decorations,
    caret_bytes: &[SourceByte],
    line_start: usize,
    line_end: usize,
) -> bool {
    // **Reveal is line-local.** A line shows raw markdown when a
    // caret sits on the same source line — nothing else. The
    // previous implementation used byte-range containment against
    // `decorations.blocks`, which had two problems on large
    // documents:
    //
    // 1. **Block flicker.** Block byte ranges shift by exactly the
    //    number of bytes typed when the decoration worker is behind
    //    the rope (a 500 KiB markdown buffer can be several
    //    revisions stale at any instant). A caret near a block
    //    boundary would then flip in-vs-out of the containing block
    //    between consecutive paints — every line in that block
    //    would toggle reveal state, the lines' wrap row counts
    //    would change, and the entire viewport would shift on every
    //    keystroke. The user-visible symptom was "as I type, all of
    //    the markdown rendering across the document toggles live
    //    and not live".
    // 2. **Containment-too-broad.** Even with fresh decorations, the
    //    rule revealed every line in the smallest containing block;
    //    a long paragraph or pipe-table block would still light up
    //    its entire row count when the caret was at a different
    //    offset inside it.
    //
    // The fix avoids both: the check uses *only* rope-derived line
    // bounds and caret bytes, so it's invariant under stale
    // decorations and tight to the single source line the caret is
    // on. Multi-line code blocks remain the one exception — those
    // reveal as a unit so the fence markers and indented body show
    // when the caret is inside, which is essential UX for editing
    // code in-place. Code-block boundaries are whole-line so
    // off-by-N-byte staleness can't flip individual lines in or out
    // of the block.
    if caret_bytes
        .iter()
        .any(|c| c.as_usize() >= line_start && c.as_usize() <= line_end)
    {
        return true;
    }
    for block in &decorations.blocks {
        if block.start_byte >= line_end || block.end_byte <= line_start {
            continue;
        }
        let reveals_as_unit = matches!(
            block.kind,
            BlockKind::FencedCodeBlock | BlockKind::IndentedCodeBlock
        );
        if reveals_as_unit && block_revealed(block, caret_bytes) {
            return true;
        }
    }
    false
}

/// Bytes of slack on either side of a checkbox span within which a caret
/// reveals the raw `[ ]` brackets.
const CHECKBOX_REVEAL_SLACK_BYTES: usize = 2;

/// Unlike the rest of a markdown line, a task checkbox reveals its raw
/// brackets only when a caret sits *on or beside the checkbox itself*,
/// not merely somewhere on the line. This lets the writer keep editing
/// the line's text with the checkbox glyph still showing, yet still edit
/// (or delete) the brackets when the caret is near them.
///
/// `span_start..span_end` is the checkbox span clamped to the line; the
/// reveal window extends [`CHECKBOX_REVEAL_SLACK_BYTES`] past each edge,
/// clamped to the line so an off-line caret never reveals it.
fn caret_near_checkbox(
    caret_bytes: &[SourceByte],
    span_start: usize,
    span_end: usize,
    line_start: usize,
    line_end: usize,
) -> bool {
    let lo = span_start
        .saturating_sub(CHECKBOX_REVEAL_SLACK_BYTES)
        .max(line_start);
    let hi = span_end
        .saturating_add(CHECKBOX_REVEAL_SLACK_BYTES)
        .min(line_end);
    caret_bytes.iter().any(|c| {
        let pos = c.as_usize();
        pos >= lo && pos <= hi
    })
}

// ---------------------------------------------------------------------------
// Segment construction
// ---------------------------------------------------------------------------

pub(super) fn build_line_segments(
    decorations: &Decorations,
    caret_bytes: &[SourceByte],
    folds: &[FoldRange],
    suppressed_table_blocks: &[Range<usize>],
    line_start: usize,
    line_end: usize,
    line_text: &str,
) -> Vec<DisplaySegment> {
    let revealed = line_revealed(decorations, caret_bytes, line_start, line_end);

    let block_style = compute_block_style(decorations, line_start, line_end, revealed);

    let mut actions: Vec<ActionRange> = Vec::new();
    let mut style_overlays: Vec<(Range<usize>, SpanStyle)> = Vec::new();
    let mut hit_overlays: Vec<(Range<usize>, SegmentHit)> = Vec::new();

    // Folds first — any fold range that intersects the line becomes Hide.
    for fold in folds {
        let s = fold.start.as_usize().max(line_start);
        let e = fold.end.as_usize().min(line_end);
        if s < e {
            actions.push(ActionRange {
                start: s,
                end: e,
                action: Action::Hide,
            });
        }
    }

    // Walk inline spans intersecting the line.
    for span in &decorations.inlines {
        let span_start = span.range.start;
        let span_end = match &span.kind {
            InlineKind::FootnoteDefinition { body_range, .. } => span.range.end.max(body_range.end),
            _ => span.range.end,
        };
        if span_end <= line_start || span_start >= line_end {
            continue;
        }
        let s = span.range.start.max(line_start);
        let e = span.range.end.min(line_end);
        if s >= e && !matches!(span.kind, InlineKind::FootnoteDefinition { .. }) {
            // Zero-length: still record the byte for Checkbox toggle / style
            // hit-test, but skip Hide/Replace.
            continue;
        }
        match &span.kind {
            InlineKind::Marker(marker) => match marker {
                MarkerKind::HeadingHash
                | MarkerKind::FenceTick
                | MarkerKind::BlockquoteCaret
                | MarkerKind::EmphasisDelim
                | MarkerKind::StrikeDelim
                | MarkerKind::CodeDelim
                | MarkerKind::ThematicBreak => {
                    if !revealed {
                        actions.push(ActionRange {
                            start: s,
                            end: e,
                            action: Action::Hide,
                        });
                    } else {
                        style_overlays.push((s..e, SpanStyle::marker()));
                    }
                }
                MarkerKind::ListMarker => {
                    // List marker spans the leading `- ` / `* ` / `1. `.
                    if !revealed {
                        let display = "• ".to_string();
                        actions.push(ActionRange {
                            start: s,
                            end: e,
                            action: Action::Replace {
                                display,
                                style: SpanStyle::bullet(),
                                hit: SegmentHit::None,
                            },
                        });
                    } else {
                        style_overlays.push((s..e, SpanStyle::marker()));
                    }
                }
                MarkerKind::TablePipe => {
                    style_overlays.push((s..e, SpanStyle::marker()));
                }
            },
            InlineKind::Strong => style_overlays.push((s..e, SpanStyle::strong())),
            InlineKind::Emphasis => style_overlays.push((s..e, SpanStyle::emphasis())),
            InlineKind::Strikethrough => style_overlays.push((s..e, SpanStyle::strike())),
            InlineKind::Code => style_overlays.push((s..e, SpanStyle::code())),
            InlineKind::Link {
                text_range,
                url_range,
            } => {
                // Style the visible text run as a link; if unrevealed,
                // hide everything outside `text_range` (i.e. the `[`,
                // `](url)` boundary chars).
                let url =
                    SourceByte::from_usize(url_range.start)..SourceByte::from_usize(url_range.end);
                let txt_s = text_range.start.max(line_start);
                let txt_e = text_range.end.min(line_end);
                if txt_s < txt_e {
                    style_overlays.push((txt_s..txt_e, SpanStyle::link()));
                    // Only a *collapsed* link is clickable-to-open. While
                    // the caret reveals the raw `[text](url)` the text is
                    // editable, so drop the hit and let a click place the
                    // caret rather than open the browser.
                    let hit = if revealed {
                        SegmentHit::None
                    } else {
                        SegmentHit::Link { url: url.clone() }
                    };
                    // Link hit metadata wraps the *visible* range; we'll
                    // promote the visible link bytes to carry a SegmentHit
                    // via a post-pass over the segment list below.
                    actions.push(ActionRange {
                        start: txt_s,
                        end: txt_e,
                        action: Action::Replace {
                            display: line_text[txt_s - line_start..txt_e - line_start].to_string(),
                            style: SpanStyle::link(),
                            hit,
                        },
                    });
                }
                if !revealed {
                    if s < txt_s {
                        actions.push(ActionRange {
                            start: s,
                            end: txt_s,
                            action: Action::Hide,
                        });
                    }
                    if txt_e < e {
                        actions.push(ActionRange {
                            start: txt_e,
                            end: e,
                            action: Action::Hide,
                        });
                    }
                }
            }
            InlineKind::FootnoteReference { label } => {
                let definition = decorations
                    .footnote_definition_for(label)
                    .map(|(range, _)| {
                        SourceByte::from_usize(range.start)..SourceByte::from_usize(range.end)
                    });
                let hit = SegmentHit::FootnoteReference {
                    label: label.clone(),
                    definition,
                };
                let span_revealed = caret_bytes
                    .iter()
                    .any(|c| c.as_usize() >= span.range.start && c.as_usize() <= span.range.end);
                if span_revealed {
                    style_overlays.push((s..e, SpanStyle::footnote()));
                    hit_overlays.push((s..e, hit));
                } else {
                    actions.push(ActionRange {
                        start: s,
                        end: e,
                        action: Action::Replace {
                            display: superscript_label(label),
                            style: SpanStyle::footnote(),
                            hit,
                        },
                    });
                }
            }
            InlineKind::FootnoteDefinition { label, body_range } => {
                let first_reference =
                    decorations
                        .footnote_first_reference_for(label)
                        .map(|range| {
                            SourceByte::from_usize(range.start)..SourceByte::from_usize(range.end)
                        });
                let hit = SegmentHit::FootnoteDefinition {
                    label: label.clone(),
                    first_reference,
                };
                if s < e {
                    style_overlays.push((s..e, SpanStyle::footnote()));
                }
                let hit_start = span.range.start.min(body_range.start).max(line_start);
                let hit_end = span.range.end.max(body_range.end).min(line_end);
                if hit_start < hit_end {
                    hit_overlays.push((hit_start..hit_end, hit));
                }
            }
            InlineKind::ImageRef { alt_range, .. } => {
                if !revealed {
                    let alt_text = if alt_range.start >= line_start && alt_range.end <= line_end {
                        line_text[alt_range.start - line_start..alt_range.end - line_start]
                            .to_string()
                    } else {
                        String::new()
                    };
                    let display = if alt_text.is_empty() {
                        "🖼".to_string()
                    } else {
                        format!("{} 🖼", alt_text)
                    };
                    actions.push(ActionRange {
                        start: s,
                        end: e,
                        action: Action::Replace {
                            display,
                            style: SpanStyle::image_label(),
                            hit: SegmentHit::None,
                        },
                    });
                }
            }
            InlineKind::Checkbox {
                checked,
                toggle_byte,
            } => {
                // The checkbox uses its own, tighter reveal rule (caret
                // near the box) rather than the line-wide `revealed`, so
                // the glyph survives while the rest of the line's text is
                // being edited.
                let checkbox_revealed =
                    caret_near_checkbox(caret_bytes, s, e, line_start, line_end);
                if !checkbox_revealed {
                    let display = if *checked {
                        "☑ ".to_string()
                    } else {
                        "☐ ".to_string()
                    };
                    actions.push(ActionRange {
                        start: s,
                        end: e,
                        action: Action::Replace {
                            display,
                            style: SpanStyle::checkbox(),
                            hit: SegmentHit::Checkbox {
                                toggle: SourceByte::from_usize(*toggle_byte),
                                checked: *checked,
                            },
                        },
                    });
                } else {
                    // Revealed: keep brackets visible, attach a no-op marker
                    // style so the renderer can still paint the toggle area
                    // dimmer.
                    style_overlays.push((s..e, SpanStyle::marker()));
                }
            }
        }
    }

    // Phase F3 — hide `==` / `{#hex:` / `}` delimiter bytes for inline
    // color spans whose `outer` range intersects the line. Caret-inside-
    // outer reveals the delimiters (mirror of the bold/italic caret-
    // reveal rule). The painted `inner` text is left intact so the
    // renderer paints over it with the resolved color.
    for ics in &decorations.inline_color_spans {
        let o_s = ics.outer.start;
        let o_e = ics.outer.end;
        if o_e <= line_start || o_s >= line_end {
            continue;
        }
        let revealed_for_span = caret_bytes
            .iter()
            .any(|c| c.as_usize() >= o_s && c.as_usize() < o_e);
        if revealed_for_span {
            continue;
        }
        // Left delimiter — `outer.start..inner.start`.
        let lhs_s = o_s.max(line_start);
        let lhs_e = ics.inner.start.min(line_end);
        if lhs_s < lhs_e {
            actions.push(ActionRange {
                start: lhs_s,
                end: lhs_e,
                action: Action::Hide,
            });
        }
        // Right delimiter — `inner.end..outer.end`.
        let rhs_s = ics.inner.end.max(line_start);
        let rhs_e = o_e.min(line_end);
        if rhs_s < rhs_e {
            actions.push(ActionRange {
                start: rhs_s,
                end: rhs_e,
                action: Action::Hide,
            });
        }
    }

    for highlight in &decorations.highlights {
        if highlight.end <= line_start || highlight.start >= line_end {
            continue;
        }
        let s = highlight.start.max(line_start);
        let e = highlight.end.min(line_end);
        if s < e {
            style_overlays.push((s..e, SpanStyle::syntax(highlight.kind)));
        }
    }

    // Pipe-table visual rendering — hide every `|` byte and the entire
    // alignment row inside every table block. The visual painter
    // (`continuity_render::table_paint`) draws cell borders + per-
    // column-aligned text on top of the source. Hides are unconditional:
    // tables are always rendered, so raw pipes are never shown.
    for range in crate::table_hide_provider::compute_table_hidden_ranges_for_line(
        decorations,
        suppressed_table_blocks,
        line_start,
        line_end,
        line_text,
    ) {
        actions.push(ActionRange {
            start: range.start,
            end: range.end,
            action: Action::Hide,
        });
    }

    // γ — backslash-escape display. `\X` where X is a markdown-escape
    // punctuation character: hide the backslash off-line, reveal it
    // when the caret returns. Sibling provider so the builder stays a
    // dispatcher rather than growing per-feature logic.
    for range in crate::backslash_escape_provider::compute_backslash_hidden_ranges_for_line(
        caret_bytes,
        line_start,
        line_end,
        line_text,
    ) {
        actions.push(ActionRange {
            start: range.start,
            end: range.end,
            action: Action::Hide,
        });
    }

    // Sort and de-overlap actions: later actions win.
    actions.sort_by_key(|a| (a.start, a.end));
    actions = dedup_overlapping_actions(actions);

    // Defensive: stale decorations (revision lag during undo replay or
    // a fresh edit) can carry byte offsets that don't land on char
    // boundaries in the *current* rope. Snap every action boundary
    // (and the style-overlay boundaries) to a valid char boundary in
    // `line_text` before composing segments, so downstream byte
    // slicing in `segment.rs::display_bytes` never panics.
    for a in &mut actions {
        a.start = snap_to_line_char_boundary(line_text, line_start, line_end, a.start);
        a.end = snap_to_line_char_boundary(line_text, line_start, line_end, a.end);
        if a.end < a.start {
            a.end = a.start;
        }
    }
    actions.retain(|a| a.end > a.start);
    for (r, _) in &mut style_overlays {
        let s = snap_to_line_char_boundary(line_text, line_start, line_end, r.start);
        let e = snap_to_line_char_boundary(line_text, line_start, line_end, r.end);
        *r = s..e.max(s);
    }
    for (r, _) in &mut hit_overlays {
        let s = snap_to_line_char_boundary(line_text, line_start, line_end, r.start);
        let e = snap_to_line_char_boundary(line_text, line_start, line_end, r.end);
        *r = s..e.max(s);
    }

    // Compose segments.
    coalesce_segments(
        line_start,
        line_end,
        line_text,
        block_style,
        &style_overlays,
        &hit_overlays,
        &actions,
    )
}

fn superscript_label(label: &str) -> String {
    let mut out = String::new();
    for ch in label.chars() {
        out.push(match ch {
            '0' => '⁰',
            '1' => '¹',
            '2' => '²',
            '3' => '³',
            '4' => '⁴',
            '5' => '⁵',
            '6' => '⁶',
            '7' => '⁷',
            '8' => '⁸',
            '9' => '⁹',
            other => other,
        });
    }
    out
}

fn compute_block_style(
    decorations: &Decorations,
    line_start: usize,
    line_end: usize,
    is_line_revealed: bool,
) -> SpanStyle {
    let mut style = SpanStyle::body();
    for b in &decorations.blocks {
        if b.start_byte >= line_end || b.end_byte <= line_start {
            continue;
        }
        match b.kind {
            BlockKind::Heading { level } | BlockKind::SetextHeading { level } => {
                let lvl = level.clamp(1, 6);
                let heading_style = if is_line_revealed {
                    SpanStyle::heading_revealed(lvl)
                } else {
                    SpanStyle::heading(lvl)
                };
                style = style.merge(heading_style);
            }
            BlockKind::FencedCodeBlock => {
                style = style.merge(SpanStyle {
                    role: SpanRole::Code,
                    ..SpanStyle::body()
                });
            }
            // IndentedCodeBlock is CommonMark's implicit "4-space-prefix
            // is code" rule — too aggressive for a notes editor where
            // indented prose is just prose. Leave the text style at body.
            BlockKind::IndentedCodeBlock => {}
            _ => {}
        }
    }
    style
}
