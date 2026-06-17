//! Tab-strip geometry: per-tab slot widths, crowding, and horizontal
//! scroll layout. Sibling of [`crate::pane_chrome`] (split out to keep
//! that file under the 600-line conventions cap).
//!
//! The painter ([`crate::pane_chrome`]) and the UI hit-test
//! (`crates/ui/src/window_mouse_tabs.rs`) both call
//! [`compute_tab_strip_metrics`] with the same `(labels, strip_w,
//! scroll_offset)` so the painted slots and the click-resolved slots
//! stay byte-identical.
//!
//! Thread ownership: pure functions over caller-owned slices; no shared
//! state.

use crate::pane_chrome::{STRIP_FONT_SIZE_DIP, TAB_PADDING_DIP};

/// Preferred (uncrowded) width cap for one tab in DIPs. A tab whose
/// estimated text width sits below this gets its estimate; a longer title
/// is capped here so one verbose tab cannot starve its neighbours.
pub const TAB_PREFERRED_WIDTH_DIP: f32 = 200.0;
/// Legacy alias kept for the non-scrolling drag-affordance callers
/// (`tab_drag_paint`, `window_tab_drag_*`) that still want a single
/// uncrowded reference width. Equal to [`TAB_PREFERRED_WIDTH_DIP`].
pub const TAB_MIN_WIDTH_DIP: f32 = TAB_PREFERRED_WIDTH_DIP;
/// Item 8 — small minimum width (DIPs) a tab may shrink to when the strip
/// is crowded, before horizontal scrolling kicks in. Below the readable
/// floor used by the wrap layout so many tabs fit a narrow strip.
pub const TAB_SHRINK_MIN_WIDTH_DIP: f32 = 88.0;
/// Item 8 — width (DIPs) reserved at each strip edge for a scroll chevron
/// (`‹` / `›`) when the tab row overflows even at the shrink minimum.
pub const TAB_CHEVRON_WIDTH_DIP: f32 = 18.0;

/// Estimated preferred width (DIPs) for a tab labelled `label`, capped at
/// [`TAB_PREFERRED_WIDTH_DIP`]. Shared by every layout path so the paint,
/// hit-test, and drag affordances agree on a tab's desired size.
#[must_use]
fn preferred_tab_width(label: &str) -> f32 {
    let chars = label.chars().count() as f32;
    let est = chars * STRIP_FONT_SIZE_DIP * 0.55 + TAB_PADDING_DIP * 2.0;
    est.min(TAB_PREFERRED_WIDTH_DIP)
}

/// Item 8 — full layout for a single (non-wrapping) tab strip row,
/// including crowding and horizontal-scroll state.
///
/// The strip is laid out in one row. When the preferred widths fit, every
/// tab gets its preferred width and the row is neither crowded nor
/// scrolling. When they don't fit, slots shrink proportionally toward
/// [`TAB_SHRINK_MIN_WIDTH_DIP`] (`crowded = true`). When even the shrink
/// minimum overflows the strip, slots stay at the shrink minimum and the
/// row scrolls horizontally (`overflowing = true`), reserving
/// [`TAB_CHEVRON_WIDTH_DIP`] at each edge for the `‹` / `›` chevrons.
///
/// `scroll_offset_dip` is the requested left scroll (clamped here to the
/// valid range). `widths` are the painted slot widths in positional order;
/// `content_x0` is the strip-relative x where the first tab's left edge is
/// painted (the chevron inset minus the clamped scroll offset, so it is
/// negative once scrolled). The same struct drives both paint and
/// hit-test so they stay byte-identical.
#[derive(Debug, Clone, PartialEq)]
pub struct TabStripMetrics {
    /// Painted slot widths in positional order.
    pub widths: Vec<f32>,
    /// Strip-relative x of the first tab's left edge (may be negative when
    /// scrolled). Tab `i`'s left edge is `content_x0 + sum(widths[..i])`.
    pub content_x0: f32,
    /// `true` when slots were shrunk below their preferred width (crowded).
    pub crowded: bool,
    /// `true` when the row overflows the strip even at the shrink minimum
    /// and is therefore horizontally scrollable.
    pub overflowing: bool,
    /// Clamped scroll offset actually applied (DIPs from the content's
    /// left). `0.0` when not overflowing.
    pub scroll_offset_dip: f32,
    /// Maximum valid scroll offset (DIPs). `0.0` when not overflowing.
    pub max_scroll_offset_dip: f32,
    /// Total `strip_w` the metrics were computed against.
    pub strip_w: f32,
}

impl TabStripMetrics {
    /// Strip-relative left edge of the left (`‹`) chevron, when overflowing.
    #[must_use]
    pub fn left_chevron_left(&self) -> f32 {
        0.0
    }

    /// Strip-relative left edge of the right (`›`) chevron, when overflowing.
    #[must_use]
    pub fn right_chevron_left(&self) -> f32 {
        (self.strip_w - TAB_CHEVRON_WIDTH_DIP).max(0.0)
    }
}

/// Item 8 — compute the single-row tab-strip layout with crowding +
/// horizontal-scroll state. `scroll_offset_dip` is clamped internally.
#[must_use]
pub fn compute_tab_strip_metrics(
    labels: &[&str],
    strip_w: f32,
    scroll_offset_dip: f32,
) -> TabStripMetrics {
    let empty = TabStripMetrics {
        widths: Vec::new(),
        content_x0: 0.0,
        crowded: false,
        overflowing: false,
        scroll_offset_dip: 0.0,
        max_scroll_offset_dip: 0.0,
        strip_w,
    };
    if labels.is_empty() || strip_w <= 0.0 {
        return empty;
    }
    let preferred: Vec<f32> = labels.iter().map(|l| preferred_tab_width(l)).collect();
    let preferred_total: f32 = preferred.iter().sum();
    // Case 1 — everything fits at preferred widths: no crowding, no scroll.
    if preferred_total <= strip_w {
        return TabStripMetrics {
            widths: preferred,
            content_x0: 0.0,
            crowded: false,
            overflowing: false,
            scroll_offset_dip: 0.0,
            max_scroll_offset_dip: 0.0,
            strip_w,
        };
    }
    // Case 2 — shrink proportionally toward the shrink minimum. If the sum
    // at the shrink minimum still fits the strip, scale to fill exactly.
    let shrink_min = TAB_SHRINK_MIN_WIDTH_DIP.min(strip_w);
    let min_total = shrink_min * labels.len() as f32;
    if min_total <= strip_w {
        // Proportional scale of the preferred widths, then floor at the
        // shrink minimum so no tab drops below the readable small width.
        let scale = strip_w / preferred_total;
        let mut widths: Vec<f32> = preferred
            .iter()
            .map(|w| (*w * scale).max(shrink_min))
            .collect();
        // Flooring at the minimum can push the row total back over strip_w;
        // trim the overshoot from the widest slots so the row still fits.
        trim_widths_to_strip(&mut widths, strip_w, shrink_min);
        return TabStripMetrics {
            widths,
            content_x0: 0.0,
            crowded: true,
            overflowing: false,
            scroll_offset_dip: 0.0,
            max_scroll_offset_dip: 0.0,
            strip_w,
        };
    }
    // Case 3 — overflow even at the shrink minimum: every tab sits at the
    // shrink minimum and the row scrolls horizontally between chevrons.
    let widths: Vec<f32> = vec![shrink_min; labels.len()];
    let content_total = min_total;
    let inset = TAB_CHEVRON_WIDTH_DIP;
    let viewport_w = (strip_w - inset * 2.0).max(0.0);
    let max_scroll = (content_total - viewport_w).max(0.0);
    let clamped = scroll_offset_dip.clamp(0.0, max_scroll);
    TabStripMetrics {
        widths,
        content_x0: inset - clamped,
        crowded: true,
        overflowing: true,
        scroll_offset_dip: clamped,
        max_scroll_offset_dip: max_scroll,
        strip_w,
    }
}

/// Reduce `widths` from the widest slots until their total fits `strip_w`,
/// never dropping any slot below `floor`. Used to absorb the rounding
/// overshoot the shrink-minimum floor introduces.
fn trim_widths_to_strip(widths: &mut [f32], strip_w: f32, floor: f32) {
    let mut total: f32 = widths.iter().sum();
    // Bounded by the slot count so a degenerate input can't loop forever.
    for _ in 0..widths.len().saturating_mul(2) {
        if total <= strip_w + 0.01 {
            return;
        }
        let Some((widest_idx, _)) = widths
            .iter()
            .enumerate()
            .filter(|(_, w)| **w > floor)
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        else {
            return;
        };
        let excess = total - strip_w;
        let headroom = widths[widest_idx] - floor;
        let cut = excess.min(headroom);
        widths[widest_idx] -= cut;
        total -= cut;
    }
}

/// Compute the width allocated to each tab in a strip of total `strip_w`
/// DIPs, ignoring horizontal scroll (offset 0).
///
/// Retained for the drag-affordance callers (`tab_drag_paint`,
/// `window_tab_drag_*`, the context menu) that want positional widths
/// without scroll state. The painter and hit-test use
/// [`compute_tab_strip_metrics`] directly so they share the scroll offset.
#[must_use]
pub fn tab_slot_widths(labels: &[&str], strip_w: f32) -> Vec<f32> {
    compute_tab_strip_metrics(labels, strip_w, 0.0).widths
}

/// Map a click x-offset (relative to the strip's left edge) to a tab index
/// using the same widths as [`tab_slot_widths`]. Returns `None` if the
/// click is past the last tab. Offset-unaware (assumes the first tab's
/// left edge is at `x = 0`); the scroll-aware path uses
/// [`tab_index_at_with_origin`].
#[must_use]
pub fn tab_index_at(widths: &[f32], x_offset: f32) -> Option<usize> {
    tab_index_at_with_origin(widths, 0.0, x_offset)
}

/// Item 8 — scroll-aware tab hit-test. `content_x0` is the strip-relative
/// x of the first tab's left edge (from [`TabStripMetrics::content_x0`]);
/// `x_offset` is the click x relative to the strip's left edge. Returns
/// `None` when the click is left of the first tab or past the last.
#[must_use]
pub fn tab_index_at_with_origin(widths: &[f32], content_x0: f32, x_offset: f32) -> Option<usize> {
    if widths.is_empty() {
        return None;
    }
    let local = x_offset - content_x0;
    if local < 0.0 {
        return None;
    }
    let mut acc = 0.0;
    for (i, w) in widths.iter().enumerate() {
        acc += *w;
        if local < acc {
            return Some(i);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_or_nonpositive_returns_empty() {
        assert!(compute_tab_strip_metrics(&[], 800.0, 0.0).widths.is_empty());
        assert!(compute_tab_strip_metrics(&["a"], 0.0, 0.0)
            .widths
            .is_empty());
    }

    #[test]
    fn fits_at_preferred_width_not_crowded() {
        let m = compute_tab_strip_metrics(&["abc", "def"], 800.0, 0.0);
        assert!(!m.crowded);
        assert!(!m.overflowing);
        assert_eq!(m.widths.len(), 2);
        // Short labels get the same below-cap preferred width.
        for w in &m.widths {
            assert!(*w <= TAB_PREFERRED_WIDTH_DIP + 0.01);
        }
    }

    #[test]
    fn crowded_shrinks_toward_small_min_without_overflow() {
        // Six full-width-preferred tabs in a strip that can hold them only
        // when shrunk: crowded but not overflowing, total fits strip.
        let labels: Vec<&str> = vec!["a-very-long-tab-title-here"; 6];
        let strip_w = 6.0 * 100.0; // 600 DIP; preferred ~200 each → 1200.
        let m = compute_tab_strip_metrics(&labels, strip_w, 0.0);
        assert!(m.crowded);
        assert!(!m.overflowing);
        let total: f32 = m.widths.iter().sum();
        assert!(total <= strip_w + 0.5, "total {total} exceeds strip");
        for w in &m.widths {
            assert!(*w >= TAB_SHRINK_MIN_WIDTH_DIP - 0.01);
        }
    }

    #[test]
    fn overflows_at_shrink_minimum_enables_scroll() {
        // Twenty tabs at the 88 DIP floor = 1760 DIP content. Strip = 300.
        let labels: Vec<&str> = vec!["t"; 20];
        let strip_w = 300.0;
        let m = compute_tab_strip_metrics(&labels, strip_w, 0.0);
        assert!(m.crowded);
        assert!(m.overflowing);
        assert!(m.max_scroll_offset_dip > 0.0);
        for w in &m.widths {
            assert!((*w - TAB_SHRINK_MIN_WIDTH_DIP).abs() < 0.01);
        }
        // content_x0 starts at the chevron inset when unscrolled.
        assert!((m.content_x0 - TAB_CHEVRON_WIDTH_DIP).abs() < 0.01);
    }

    #[test]
    fn scroll_offset_is_clamped_and_shifts_content_origin() {
        let labels: Vec<&str> = vec!["t"; 20];
        let strip_w = 300.0;
        let unclamped = compute_tab_strip_metrics(&labels, strip_w, 1.0e6);
        assert_eq!(unclamped.scroll_offset_dip, unclamped.max_scroll_offset_dip);
        // content_x0 = inset - clamped_offset → far left once fully scrolled.
        assert!(unclamped.content_x0 < 0.0);
        let negative = compute_tab_strip_metrics(&labels, strip_w, -50.0);
        assert_eq!(negative.scroll_offset_dip, 0.0);
    }

    #[test]
    fn hit_test_with_origin_matches_painted_slots() {
        let labels: Vec<&str> = vec!["t"; 20];
        let m = compute_tab_strip_metrics(&labels, 300.0, 100.0);
        // The first visible tab under the left chevron edge.
        let first_visible_x = TAB_CHEVRON_WIDTH_DIP + 1.0;
        let idx = tab_index_at_with_origin(&m.widths, m.content_x0, first_visible_x);
        assert!(idx.is_some());
        // A click left of the (scrolled) content origin returns None.
        assert!(tab_index_at_with_origin(&m.widths, m.content_x0, m.content_x0 - 5.0).is_none());
    }

    #[test]
    fn tab_index_at_unscrolled_picks_correct_slot() {
        let widths = [100.0, 100.0, 100.0];
        assert_eq!(tab_index_at(&widths, 50.0), Some(0));
        assert_eq!(tab_index_at(&widths, 150.0), Some(1));
        assert_eq!(tab_index_at(&widths, 250.0), Some(2));
        assert_eq!(tab_index_at(&widths, 350.0), None);
        assert_eq!(tab_index_at(&widths, -1.0), None);
    }
}
