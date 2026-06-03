//! Segment-assembly: turn a source line + decorations + caret + folds
//! into the ordered `DisplaySegment` list that the renderer paints.
//! Extracted from `crates/display_map/src/builder.rs` to keep the
//! parent file under the 600-line conventions cap.

use continuity_decorate::{Decorations, InlineColorKind, InlineKind, MarkerKind};
use std::ops::Range;

use super::segment_coalescing::{
    coalesce_segments, dedup_overlapping_actions, snap_to_line_char_boundary, Action, ActionRange,
};
use super::segments_helpers::{
    caret_near_checkbox, collect_toggle_off_emphasis_ranges, compute_block_style, line_revealed,
    range_touches_kept, superscript_label,
};
use crate::fold::FoldRange;
use crate::id::SourceByte;
use crate::markdown_toggles::MarkdownRenderToggles;
use crate::segment::{DisplaySegment, SegmentHit};
use crate::style::SpanStyle;

// ---------------------------------------------------------------------------
// Segment construction
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub(super) fn build_line_segments(
    decorations: &Decorations,
    caret_bytes: &[SourceByte],
    folds: &[FoldRange],
    suppressed_table_blocks: &[Range<usize>],
    toggles: MarkdownRenderToggles,
    line_start: usize,
    line_end: usize,
    line_text: &str,
) -> Vec<DisplaySegment> {
    let revealed = line_revealed(decorations, caret_bytes, line_start, line_end);

    let block_style = compute_block_style(decorations, line_start, line_end, revealed, toggles);

    // Pre-pass: collect the clamped body byte ranges of emphasis / strong
    // spans whose render toggle is OFF. Their `*` / `**` delimiters must
    // stay visible (raw markdown) even when the line is not caret-
    // revealed, so the `EmphasisDelim` hide arm below skips any delimiter
    // adjacent to one of these ranges. `EmphasisDelim` markers are shared
    // between italic and bold (same `MarkerKind`), so on a mixed
    // `*i* **b**` line with italic-off / bold-on the `*` reveals while
    // the `**` still hides.
    let keep_visible_emphasis_delims =
        collect_toggle_off_emphasis_ranges(decorations, toggles, line_start, line_end);

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
                | MarkerKind::StrikeDelim
                | MarkerKind::CodeDelim => {
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
                MarkerKind::EmphasisDelim => {
                    // Keep the `*` / `**` delimiter visible (raw markdown)
                    // when its emphasis kind is toggled off, mirroring the
                    // caret-reveal path. Otherwise hide it off-line and
                    // style it when revealed.
                    let force_visible = range_touches_kept(&keep_visible_emphasis_delims, s, e);
                    if revealed || force_visible {
                        style_overlays.push((s..e, SpanStyle::marker()));
                    } else {
                        actions.push(ActionRange {
                            start: s,
                            end: e,
                            action: Action::Hide,
                        });
                    }
                }
                MarkerKind::ThematicBreak => {
                    // `render_divider` OFF leaves the literal `---` / `***`
                    // / `___` chars visible (no hide, no marker style); the
                    // render side likewise skips `paint_horizontal_rules`.
                    if !toggles.divider {
                        // Raw markdown: do nothing — bytes render as body.
                    } else if !revealed {
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
            InlineKind::Strong => {
                // `render_bold` OFF: no strong styling overlay, so the
                // body renders unstyled and the kept-visible `**`
                // delimiters above show as literal markup.
                if toggles.bold {
                    style_overlays.push((s..e, SpanStyle::strong()));
                }
            }
            InlineKind::Emphasis => {
                // `render_italic` OFF (the shipped default): no emphasis
                // styling overlay, literal `*` markers stay visible.
                if toggles.italic {
                    style_overlays.push((s..e, SpanStyle::emphasis()));
                }
            }
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
        // `render_highlight` OFF leaves `==text==` delimiters visible
        // (raw markdown). Gate ONLY the `Highlight` kind: `{#hex:}` color
        // shares this pass but is a separate feature and stays working.
        if matches!(ics.kind, InlineColorKind::Highlight) && !toggles.highlight {
            continue;
        }
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
