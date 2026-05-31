//! Phase F1 — sticky heading breadcrumb at pane top.
//!
//! A thin bar pinned beneath the pane chrome and above the editor body.
//! Renders the heading chain enclosing the visible top of the viewport
//! (`H2 title › H3 title › H4 title`) and truncates the middle with `…`
//! when the chain would overflow the pane width. Each rendered heading
//! segment carries its source-byte target so the UI hit-test can map a
//! click back to a scroll-into-view action.
//!
//! This module is pure layout + types: no D2D / DirectWrite calls. The
//! UI orchestrator builds a [`BreadcrumbData`] from the active buffer's
//! heading list (via [`continuity_decorate::sections::heading_chain_at`])
//! and the renderer paints it from there. Hit-tests use
//! [`BreadcrumbLayout`] returned by [`compute_breadcrumb_layout`].
//!
//! Thread ownership: UI thread of the owning window (caller). The data
//! types are plain values so they can be built off-thread if a future
//! pipeline wants to.

use crate::params::Rgba;

/// Breadcrumb strip height in DIPs. Picked to match the status-bar
/// strip so a future "breadcrumb on top, status bar on bottom" frame
/// reads symmetrically.
pub const BREADCRUMB_HEIGHT_DIP: f32 = 22.0;

/// Inner left/right padding for the breadcrumb text run, in DIPs.
pub(crate) const BAR_EDGE_PAD_DIP: f32 = 8.0;

/// Padding between a heading segment and its neighbouring `›` glyph.
pub(crate) const SEGMENT_GAP_DIP: f32 = 6.0;

/// Default separator glyph rendered between heading segments. `›` was
/// picked to match the spec example in `roadmap_v2.md §F1`.
pub(crate) const SEPARATOR_GLYPH: &str = "\u{203A}";

/// Ellipsis glyph rendered when middle segments are truncated.
pub(crate) const ELLIPSIS_GLYPH: &str = "\u{2026}";

/// One heading in the breadcrumb chain. Built by the UI from
/// [`continuity_decorate::sections::heading_chain_at`] — the chain is
/// ordered outermost-first (H1 → … → caret-section heading).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BreadcrumbSegment {
    /// Heading text (`# Foo` is stored as `Foo`).
    pub text: String,
    /// Heading level (1..=6). Carried through for click-target styling.
    pub level: u8,
    /// Source-byte offset of the heading line's start in the rope. The
    /// click handler uses this to scroll the heading line into view.
    pub target_byte: u32,
}

/// Theme-derived breadcrumb colors. Three keys per spec §F1:
/// `editor.breadcrumb.{foreground, separator, active}`.
#[derive(Copy, Clone, Debug, Default)]
pub struct BreadcrumbColors {
    /// `editor.breadcrumb.foreground` — base segment text color.
    pub fg: Rgba,
    /// `editor.breadcrumb.separator` — `›` separator glyph color.
    pub separator: Rgba,
    /// `editor.breadcrumb.active` — innermost (current) segment color.
    pub active: Rgba,
}

/// All data the painter needs to lay out + render a single pane's
/// breadcrumb strip.
#[derive(Clone, Debug)]
pub struct BreadcrumbData<'a> {
    /// Heading chain, outermost first. Empty ⇒ the painter renders only
    /// the strip background (or skips entirely if disabled).
    pub segments: &'a [BreadcrumbSegment],
    /// Theme colors.
    pub colors: BreadcrumbColors,
    /// Font size in DIPs used for the segment text + width estimate.
    pub font_size_dip: f32,
}

/// What a [`SlotBounds`] points to in the original segment list. The
/// click handler routes accordingly.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SlotKind {
    /// Heading segment at the given index into
    /// [`BreadcrumbData::segments`]. Click → scroll to the segment's
    /// `target_byte`.
    Heading {
        /// Index into [`BreadcrumbData::segments`].
        index: u32,
    },
    /// Separator glyph between two adjacent headings. Not clickable.
    Separator,
    /// Middle-truncation ellipsis. Not clickable.
    Ellipsis,
}

/// One painted slot inside the breadcrumb strip with its DIP bounds and
/// a tag mapping back to the source data.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SlotBounds {
    /// Left edge in viewport DIPs.
    pub left: f32,
    /// Right edge in viewport DIPs.
    pub right: f32,
    /// What this slot represents.
    pub kind: SlotKind,
}

/// Per-frame breadcrumb layout. Empty when the breadcrumb is hidden or
/// the chain is empty.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BreadcrumbLayout {
    /// Y top of the strip in viewport DIPs.
    pub top: f32,
    /// Per-slot bounds in paint order (left-to-right).
    pub slots: Vec<SlotBounds>,
}

impl BreadcrumbLayout {
    /// Hit-test a viewport-relative `(x, y)` point. Returns the heading
    /// index whose slot contains the point, or `None` when the point is
    /// outside every clickable slot (including separator / ellipsis
    /// slots, which are deliberately non-clickable).
    #[must_use]
    pub fn heading_at(&self, x: f32, y: f32) -> Option<u32> {
        if y < self.top || y > self.top + BREADCRUMB_HEIGHT_DIP {
            return None;
        }
        for slot in &self.slots {
            if x >= slot.left && x <= slot.right {
                if let SlotKind::Heading { index } = slot.kind {
                    return Some(index);
                }
            }
        }
        None
    }
}

/// Pre-measure a segment's width using a monospace approximation. The
/// renderer uses this so paint and hit-test agree without a live D2D
/// context. Matches the same `font_size_dip * 0.55` heuristic that the
/// status bar already uses.
#[must_use]
pub fn estimate_text_width_dip(text: &str, font_size_dip: f32) -> f32 {
    let advance = font_size_dip * 0.55;
    (text.chars().count() as f32) * advance
}

/// Build a [`BreadcrumbLayout`] for the supplied segments without
/// painting.
///
/// Layout rule:
/// - Segments laid out left-to-right with `›` separators between them
///   and [`SEGMENT_GAP_DIP`] padding either side of each separator.
/// - When the full chain would overflow `viewport_w - 2 * BAR_EDGE_PAD_DIP`,
///   the **middle** segments are replaced with a single non-clickable
///   `…` slot. The outermost (first) and innermost (last) segments are
///   always preserved when the chain has length ≥ 2; for length 1 the
///   single segment is preserved and clipped to the bar width.
#[must_use]
pub fn compute_breadcrumb_layout(
    data: &BreadcrumbData<'_>,
    viewport_w: f32,
    top: f32,
) -> BreadcrumbLayout {
    if data.segments.is_empty() || viewport_w <= 2.0 * BAR_EDGE_PAD_DIP {
        return BreadcrumbLayout {
            top,
            slots: Vec::new(),
        };
    }
    let max_inner = (viewport_w - 2.0 * BAR_EDGE_PAD_DIP).max(0.0);
    let font = data.font_size_dip;

    let sep_w = estimate_text_width_dip(SEPARATOR_GLYPH, font);
    let ellipsis_w = estimate_text_width_dip(ELLIPSIS_GLYPH, font);
    let seg_widths: Vec<f32> = data
        .segments
        .iter()
        .map(|s| estimate_text_width_dip(&s.text, font))
        .collect();

    // Width if every segment + separator fits.
    let full_width = full_chain_width(&seg_widths, sep_w);

    let slots = if full_width <= max_inner {
        layout_full_chain(&seg_widths, sep_w)
    } else if data.segments.len() == 1 {
        // Single segment — keep it; layout clips at the edge.
        vec![SlotBounds {
            left: BAR_EDGE_PAD_DIP,
            right: BAR_EDGE_PAD_DIP + seg_widths[0],
            kind: SlotKind::Heading { index: 0 },
        }]
    } else {
        layout_with_middle_truncation(&seg_widths, sep_w, ellipsis_w, max_inner)
    };

    // Shift `slots` (currently zero-origined) by the edge padding.
    let slots: Vec<SlotBounds> = slots
        .into_iter()
        .map(|s| SlotBounds {
            left: s.left + BAR_EDGE_PAD_DIP,
            right: s.right + BAR_EDGE_PAD_DIP,
            kind: s.kind,
        })
        .collect();

    BreadcrumbLayout { top, slots }
}

fn full_chain_width(seg_widths: &[f32], sep_w: f32) -> f32 {
    let n = seg_widths.len();
    if n == 0 {
        return 0.0;
    }
    let seg_total: f32 = seg_widths.iter().sum();
    let sep_total = (n - 1) as f32 * (sep_w + 2.0 * SEGMENT_GAP_DIP);
    seg_total + sep_total
}

fn layout_full_chain(seg_widths: &[f32], sep_w: f32) -> Vec<SlotBounds> {
    let mut slots = Vec::with_capacity(seg_widths.len() * 2);
    let mut cursor = 0.0_f32;
    for (i, w) in seg_widths.iter().enumerate() {
        if i > 0 {
            cursor += SEGMENT_GAP_DIP;
            slots.push(SlotBounds {
                left: cursor,
                right: cursor + sep_w,
                kind: SlotKind::Separator,
            });
            cursor += sep_w + SEGMENT_GAP_DIP;
        }
        slots.push(SlotBounds {
            left: cursor,
            right: cursor + *w,
            kind: SlotKind::Heading { index: i as u32 },
        });
        cursor += *w;
    }
    slots
}

/// Truncate by replacing the middle of the chain with a single `…`. The
/// first + last segments are always preserved (length ≥ 2 is the
/// caller's precondition); intermediate segments are dropped greedily
/// from the outermost-side inward until the layout fits.
fn layout_with_middle_truncation(
    seg_widths: &[f32],
    sep_w: f32,
    ellipsis_w: f32,
    max_inner: f32,
) -> Vec<SlotBounds> {
    let n = seg_widths.len();
    let first_w = seg_widths[0];
    let last_w = seg_widths[n - 1];
    let pair_sep_block = sep_w + 2.0 * SEGMENT_GAP_DIP;

    // Width of the `first › … › last` baseline (always shown when
    // truncating).
    let baseline = first_w + 2.0 * pair_sep_block + ellipsis_w + last_w;

    // If even the baseline doesn't fit, lay out `first › … › last` and
    // let the painter clip to the strip width — the ellipsis still
    // signals that middle segments were elided. This matches the F1
    // contract ("truncated middle if overflows pane width") for
    // narrow panes that can't even host the baseline.
    if baseline > max_inner {
        let mut slots = Vec::with_capacity(5);
        let mut cursor = 0.0_f32;
        slots.push(SlotBounds {
            left: cursor,
            right: cursor + first_w,
            kind: SlotKind::Heading { index: 0 },
        });
        cursor += first_w + SEGMENT_GAP_DIP;
        slots.push(SlotBounds {
            left: cursor,
            right: cursor + sep_w,
            kind: SlotKind::Separator,
        });
        cursor += sep_w + SEGMENT_GAP_DIP;
        slots.push(SlotBounds {
            left: cursor,
            right: cursor + ellipsis_w,
            kind: SlotKind::Ellipsis,
        });
        cursor += ellipsis_w + SEGMENT_GAP_DIP;
        slots.push(SlotBounds {
            left: cursor,
            right: cursor + sep_w,
            kind: SlotKind::Separator,
        });
        cursor += sep_w + SEGMENT_GAP_DIP;
        slots.push(SlotBounds {
            left: cursor,
            right: cursor + last_w,
            kind: SlotKind::Heading {
                index: (n - 1) as u32,
            },
        });
        return slots;
    }

    // Greedily try to keep more middle segments as long as they fit
    // alongside the baseline. We re-add segments outermost-side-first
    // so the chain reads `outer › … › kept-middle › last`.
    //
    // Order of expansion: start with [0, last], then probe inserting
    // {1, 2, …, n-2} between the ellipsis and `last`.
    let mut kept_middle: Vec<usize> = Vec::new();
    let mut budget_used = baseline;
    #[allow(clippy::needless_range_loop)]
    // index is used to populate kept_middle, not just for indexing.
    for i in 1..n - 1 {
        // Adding segment i means: replace one ellipsis-adjacent gap
        // with `seg_i + (sep+gap*2)` and keep the ellipsis if anything
        // remains beyond i.
        let added = seg_widths[i] + pair_sep_block;
        if budget_used + added <= max_inner {
            kept_middle.push(i);
            budget_used += added;
        } else {
            break;
        }
    }

    let mut slots: Vec<SlotBounds> = Vec::with_capacity(8);
    let mut cursor = 0.0_f32;

    // First segment.
    slots.push(SlotBounds {
        left: cursor,
        right: cursor + first_w,
        kind: SlotKind::Heading { index: 0 },
    });
    cursor += first_w;

    // The ellipsis appears between (last preserved outer index) and
    // (first preserved inner index) — if kept_middle still leaves at
    // least one dropped segment between itself and `last`, otherwise
    // the chain is contiguous and no ellipsis is needed.
    let kept_max = kept_middle.last().copied().unwrap_or(0);
    let needs_ellipsis = kept_max < n - 2;

    if needs_ellipsis {
        // separator + ellipsis + separator.
        cursor += SEGMENT_GAP_DIP;
        slots.push(SlotBounds {
            left: cursor,
            right: cursor + sep_w,
            kind: SlotKind::Separator,
        });
        cursor += sep_w + SEGMENT_GAP_DIP;
        slots.push(SlotBounds {
            left: cursor,
            right: cursor + ellipsis_w,
            kind: SlotKind::Ellipsis,
        });
        cursor += ellipsis_w;
    }

    for &mid in &kept_middle {
        cursor += SEGMENT_GAP_DIP;
        slots.push(SlotBounds {
            left: cursor,
            right: cursor + sep_w,
            kind: SlotKind::Separator,
        });
        cursor += sep_w + SEGMENT_GAP_DIP;
        let w = seg_widths[mid];
        slots.push(SlotBounds {
            left: cursor,
            right: cursor + w,
            kind: SlotKind::Heading { index: mid as u32 },
        });
        cursor += w;
    }

    // Final separator + last segment.
    cursor += SEGMENT_GAP_DIP;
    slots.push(SlotBounds {
        left: cursor,
        right: cursor + sep_w,
        kind: SlotKind::Separator,
    });
    cursor += sep_w + SEGMENT_GAP_DIP;
    slots.push(SlotBounds {
        left: cursor,
        right: cursor + last_w,
        kind: SlotKind::Heading {
            index: (n - 1) as u32,
        },
    });

    slots
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str, level: u8, target: u32) -> BreadcrumbSegment {
        BreadcrumbSegment {
            text: text.into(),
            level,
            target_byte: target,
        }
    }

    fn data<'a>(segments: &'a [BreadcrumbSegment]) -> BreadcrumbData<'a> {
        BreadcrumbData {
            segments,
            colors: BreadcrumbColors::default(),
            font_size_dip: 14.0,
        }
    }

    #[test]
    fn empty_chain_yields_empty_layout() {
        let segs: Vec<BreadcrumbSegment> = Vec::new();
        let layout = compute_breadcrumb_layout(&data(&segs), 800.0, 0.0);
        assert!(layout.slots.is_empty());
    }

    #[test]
    fn full_chain_fits_lays_out_with_separators() {
        let segs = vec![seg("A", 1, 0), seg("B", 2, 16), seg("C", 3, 32)];
        let layout = compute_breadcrumb_layout(&data(&segs), 800.0, 4.0);
        // 3 heading slots + 2 separator slots.
        let heading_slots: Vec<_> = layout
            .slots
            .iter()
            .filter_map(|s| match s.kind {
                SlotKind::Heading { index } => Some(index),
                _ => None,
            })
            .collect();
        assert_eq!(heading_slots, vec![0, 1, 2]);
        let sep_count = layout
            .slots
            .iter()
            .filter(|s| matches!(s.kind, SlotKind::Separator))
            .count();
        assert_eq!(sep_count, 2);
        assert_eq!(layout.top, 4.0);
        // Slots are laid out in strictly increasing left edge.
        let mut last = -1.0_f32;
        for s in &layout.slots {
            assert!(s.left >= last);
            assert!(s.right > s.left);
            last = s.left;
        }
    }

    #[test]
    fn middle_truncation_keeps_first_and_last() {
        let labels = ["root", "alpha", "beta", "gamma", "delta", "leaf"];
        let segs: Vec<_> = labels
            .iter()
            .enumerate()
            .map(|(i, t)| seg(t, (i + 1) as u8, (i * 8) as u32))
            .collect();
        // Narrow bar forces truncation.
        let layout = compute_breadcrumb_layout(&data(&segs), 70.0, 0.0);
        let heading_indices: Vec<u32> = layout
            .slots
            .iter()
            .filter_map(|s| match s.kind {
                SlotKind::Heading { index } => Some(index),
                _ => None,
            })
            .collect();
        // First and last preserved.
        assert_eq!(heading_indices.first().copied(), Some(0));
        assert_eq!(heading_indices.last().copied(), Some(5));
        // Ellipsis present.
        assert!(layout
            .slots
            .iter()
            .any(|s| matches!(s.kind, SlotKind::Ellipsis)));
    }

    #[test]
    fn single_segment_chain_is_preserved() {
        let segs = vec![seg("only", 1, 0)];
        let layout = compute_breadcrumb_layout(&data(&segs), 800.0, 0.0);
        assert_eq!(layout.slots.len(), 1);
        assert!(matches!(
            layout.slots[0].kind,
            SlotKind::Heading { index: 0 }
        ));
    }

    #[test]
    fn hit_test_returns_clicked_heading_index() {
        let segs = vec![seg("A", 1, 0), seg("B", 2, 16), seg("C", 3, 32)];
        let layout = compute_breadcrumb_layout(&data(&segs), 800.0, 4.0);
        // Pick the first heading slot's midpoint.
        let first = layout
            .slots
            .iter()
            .find(|s| matches!(s.kind, SlotKind::Heading { index: 0 }))
            .unwrap();
        let mid_x = (first.left + first.right) / 2.0;
        let mid_y = layout.top + BREADCRUMB_HEIGHT_DIP / 2.0;
        assert_eq!(layout.heading_at(mid_x, mid_y), Some(0));
    }

    #[test]
    fn hit_test_outside_strip_returns_none() {
        let segs = vec![seg("A", 1, 0), seg("B", 2, 16)];
        let layout = compute_breadcrumb_layout(&data(&segs), 800.0, 50.0);
        assert!(layout.heading_at(0.0, 0.0).is_none());
        assert!(layout
            .heading_at(0.0, 50.0 + BREADCRUMB_HEIGHT_DIP + 5.0)
            .is_none());
    }

    #[test]
    fn hit_test_on_separator_is_not_clickable() {
        let segs = vec![seg("A", 1, 0), seg("B", 2, 16)];
        let layout = compute_breadcrumb_layout(&data(&segs), 800.0, 0.0);
        let sep = layout
            .slots
            .iter()
            .find(|s| matches!(s.kind, SlotKind::Separator))
            .unwrap();
        let mid_x = (sep.left + sep.right) / 2.0;
        let mid_y = layout.top + BREADCRUMB_HEIGHT_DIP / 2.0;
        assert_eq!(layout.heading_at(mid_x, mid_y), None);
    }

    #[test]
    fn tiny_viewport_falls_back_to_first_and_last_only() {
        let labels = ["one", "two", "three", "four"];
        let segs: Vec<_> = labels
            .iter()
            .enumerate()
            .map(|(i, t)| seg(t, (i + 1) as u8, (i * 8) as u32))
            .collect();
        // Below the baseline width for all but the last fallback path.
        let layout = compute_breadcrumb_layout(&data(&segs), 30.0, 0.0);
        let heading_indices: Vec<u32> = layout
            .slots
            .iter()
            .filter_map(|s| match s.kind {
                SlotKind::Heading { index } => Some(index),
                _ => None,
            })
            .collect();
        // First + last preserved even when full baseline can't fit.
        assert_eq!(heading_indices, vec![0, (segs.len() - 1) as u32]);
    }

    #[test]
    fn zero_width_viewport_yields_empty_layout() {
        let segs = vec![seg("A", 1, 0)];
        let layout = compute_breadcrumb_layout(&data(&segs), 0.0, 0.0);
        assert!(layout.slots.is_empty());
    }
}
