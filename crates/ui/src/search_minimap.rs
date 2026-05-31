//! G4 — pure layout math for the search-active minimap strip.
//!
//! The strip is a thin vertical column docked on the right edge of an
//! editor pane while the find bar is open. It paints the pane
//! background with a faint tint, then one short colored tick per find
//! match, plus a distinct tick at the currently-focused match. The
//! actual painting lives in `crates/render` — this module owns only
//! the geometry: where each tick goes, what the strip rect is, and
//! which match a click maps to.
//!
//! Kept tiny + pure so a unit test can verify every branch without
//! spinning up DirectWrite or a `Window`. The result struct is what
//! the painter consumes.

use continuity_render::{
    EditorColors, SearchHighlightRangeDraw, SearchMinimapDraw, SearchMinimapTickDraw,
};
use continuity_search::MatchRange;

/// Width of the strip in DIPs. Spec says "thin"; this matches the
/// minimap-off scroll-bar gutter width so it visually replaces it
/// during search.
pub const SEARCH_MINIMAP_WIDTH_DIP: f32 = 12.0;

/// Tick height in DIPs. Wide enough to read against the strip, tall
/// enough to register on a dense buffer.
pub const SEARCH_MINIMAP_TICK_HEIGHT_DIP: f32 = 2.0;

/// One tick on the minimap strip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MinimapTick {
    /// Top edge of the tick in DIPs, relative to the strip's top.
    pub y_dip: f32,
    /// Height in DIPs.
    pub height_dip: f32,
    /// Index into the find bar's `matches` vec — what a click resolves
    /// to.
    pub match_index: usize,
    /// `true` for the currently-focused match; the painter uses a
    /// distinct color and a wider tick.
    pub is_active: bool,
}

/// Strip layout — what the painter draws, in pane-local DIPs.
#[derive(Debug, Clone, PartialEq)]
pub struct MinimapLayout {
    /// Left edge of the strip.
    pub x_dip: f32,
    /// Top edge of the strip (typically pane top).
    pub y_dip: f32,
    /// Width of the strip.
    pub width_dip: f32,
    /// Height of the strip (typically pane height).
    pub height_dip: f32,
    /// One paint tick per match, or per vertical bucket when a result
    /// set is denser than the strip can display. Empty when `matches`
    /// is empty.
    pub ticks: Vec<MinimapTick>,
}

/// Build the strip layout for a pane.
///
/// - `pane_w_dip` / `pane_h_dip` — pane content rect dimensions.
/// - `total_lines` — total lines in the source buffer (≥ 1).
/// - `matches` — the find bar's current match list (in source order).
/// - `current_match` — index into `matches` for the focused match.
/// - `right_inset_dip` — DIPs reserved on the right edge of the pane
///   by an outer chrome consumer (currently the outline sidebar). The
///   strip docks at `pane_w_dip - right_inset_dip - width`, so when an
///   outline is also active the two columns do not overlap. Pass
///   `0.0` when no other right-edge consumer is active.
#[must_use]
pub fn build_layout(
    pane_w_dip: f32,
    pane_h_dip: f32,
    total_lines: u64,
    matches: &[MatchRange],
    current_match: usize,
    right_inset_dip: f32,
) -> MinimapLayout {
    let inset = right_inset_dip.max(0.0);
    let avail = (pane_w_dip - inset).max(0.0);
    let width = SEARCH_MINIMAP_WIDTH_DIP.min(avail);
    let x = (pane_w_dip - inset - width).max(0.0);
    let height = pane_h_dip.max(0.0);
    let total = total_lines.max(1);
    let lines = total as f32;
    let ticks = build_ticks(height, total, lines, matches, current_match);
    MinimapLayout {
        x_dip: x,
        y_dip: 0.0,
        width_dip: width,
        height_dip: height,
        ticks,
    }
}

fn build_ticks(
    height: f32,
    total_lines: u64,
    lines: f32,
    matches: &[MatchRange],
    current_match: usize,
) -> Vec<MinimapTick> {
    let max_visible_ticks = height.ceil().max(1.0) as usize;
    if matches.len() <= max_visible_ticks {
        let mut ticks = Vec::with_capacity(matches.len());
        for (i, m) in matches.iter().enumerate() {
            ticks.push(match_tick(height, total_lines, lines, i, m, current_match));
        }
        return ticks;
    }
    let mut buckets = vec![None; max_visible_ticks];
    let last_bucket = max_visible_ticks.saturating_sub(1);
    for (i, m) in matches.iter().enumerate() {
        let tick = match_tick(height, total_lines, lines, i, m, current_match);
        let bucket = tick.y_dip.floor().max(0.0).min(last_bucket as f32) as usize;
        if tick.is_active || buckets[bucket].is_none() {
            buckets[bucket] = Some(tick);
        }
    }
    buckets.into_iter().flatten().collect()
}

fn match_tick(
    height: f32,
    total_lines: u64,
    lines: f32,
    index: usize,
    match_range: &MatchRange,
    current_match: usize,
) -> MinimapTick {
    // 1-indexed `line` from MatchRange. Clamp to `total_lines` so a
    // stale match against a shrunken buffer still maps to a valid tick
    // rather than overflowing.
    let line = (match_range.line.max(1).min(total_lines)) as f32 - 1.0;
    let y = (line / lines) * height;
    MinimapTick {
        y_dip: y,
        height_dip: SEARCH_MINIMAP_TICK_HEIGHT_DIP,
        match_index: index,
        is_active: index == current_match,
    }
}

/// Resolve a `(x, y)` click in pane-local DIPs to the nearest tick's
/// match index, or `None` when the click misses the strip or there are
/// no ticks within the hit slop.
///
/// `slop_dip` — vertical hit-test fudge so the user doesn't have to
/// pixel-precisely target a 2-DIP tick.
#[must_use]
pub fn hit_test(layout: &MinimapLayout, x_dip: f32, y_dip: f32, slop_dip: f32) -> Option<usize> {
    if x_dip < layout.x_dip || x_dip > layout.x_dip + layout.width_dip {
        return None;
    }
    let mut best: Option<(usize, f32)> = None;
    for t in &layout.ticks {
        // Compare against the tick's vertical center for symmetric slop.
        let center = layout.y_dip + t.y_dip + t.height_dip * 0.5;
        let d = (y_dip - center).abs();
        if d <= slop_dip && best.is_none_or(|(_, db)| d < db) {
            best = Some((t.match_index, d));
        }
    }
    best.map(|(i, _)| i)
}

/// Phase G4 — project the pure-layout `MinimapLayout` to the
/// renderer's `SearchMinimapDraw` payload, sourcing the three strip
/// colors from the active theme's `EditorColors`.
#[must_use]
pub fn project_to_draw(
    layout: &MinimapLayout,
    colors: &EditorColors,
    matches: &[MatchRange],
    current_match: usize,
) -> SearchMinimapDraw {
    SearchMinimapDraw {
        x_dip: layout.x_dip,
        y_dip: layout.y_dip,
        width_dip: layout.width_dip,
        height_dip: layout.height_dip,
        ticks: layout
            .ticks
            .iter()
            .map(|t| SearchMinimapTickDraw {
                y_dip: t.y_dip,
                height_dip: t.height_dip,
                is_active: t.is_active,
            })
            .collect(),
        bg: colors.search_minimap_bg,
        match_color: colors.search_minimap_match,
        match_active: colors.search_minimap_match_active,
        body_highlights: matches
            .iter()
            .enumerate()
            .map(|(index, range)| SearchHighlightRangeDraw {
                start_byte: range.start_byte,
                end_byte: range.end_byte,
                is_active: index == current_match,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(line: u64) -> MatchRange {
        MatchRange {
            line,
            start_byte: 0,
            end_byte: 1,
        }
    }

    #[test]
    fn empty_match_list_yields_strip_with_no_ticks() {
        let l = build_layout(800.0, 600.0, 100, &[], 0, 0.0);
        assert_eq!(l.width_dip, SEARCH_MINIMAP_WIDTH_DIP);
        assert_eq!(l.x_dip, 800.0 - SEARCH_MINIMAP_WIDTH_DIP);
        assert!(l.ticks.is_empty());
    }

    #[test]
    fn ticks_distribute_proportional_to_line_count() {
        let matches = vec![m(1), m(50), m(100)];
        let l = build_layout(800.0, 600.0, 100, &matches, 1, 0.0);
        assert_eq!(l.ticks.len(), 3);
        // Line 1 → top; line 50 → ~middle; line 100 → ~bottom.
        assert!((l.ticks[0].y_dip - 0.0).abs() < 0.1);
        assert!((l.ticks[1].y_dip - 294.0).abs() < 1.0); // 49/100 * 600
        assert!((l.ticks[2].y_dip - 594.0).abs() < 1.0); // 99/100 * 600
        assert!(l.ticks[1].is_active);
        assert!(!l.ticks[0].is_active);
        assert!(!l.ticks[2].is_active);
    }

    #[test]
    fn strip_clamps_to_narrow_pane() {
        let l = build_layout(8.0, 600.0, 100, &[m(1)], 0, 0.0);
        // Pane narrower than strip width → strip uses the pane width.
        assert!(l.width_dip <= 8.0);
        assert_eq!(l.x_dip, 0.0);
    }

    #[test]
    fn outline_inset_shifts_strip_inward() {
        // An outline sidebar reserves 220 DIP on the right; the search
        // strip must inset by that much so the two columns do not stack
        // on the same pixels.
        let l = build_layout(800.0, 600.0, 100, &[m(1)], 0, 220.0);
        assert_eq!(l.width_dip, SEARCH_MINIMAP_WIDTH_DIP);
        assert_eq!(l.x_dip, 800.0 - 220.0 - SEARCH_MINIMAP_WIDTH_DIP);
    }

    #[test]
    fn hit_test_resolves_click_within_slop() {
        let matches = vec![m(10), m(20), m(30)];
        let l = build_layout(800.0, 600.0, 100, &matches, 0, 0.0);
        let strip_x = l.x_dip + 4.0;
        // Click on the strip near tick 1 (line 20 → y ~114).
        let y = l.ticks[1].y_dip + l.ticks[1].height_dip * 0.5;
        assert_eq!(hit_test(&l, strip_x, y, 6.0), Some(1));
        // Click well outside any tick's slop returns None.
        assert_eq!(hit_test(&l, strip_x, y + 50.0, 4.0), None);
        // Click outside the strip horizontally returns None.
        assert_eq!(hit_test(&l, l.x_dip - 5.0, y, 4.0), None);
    }

    #[test]
    fn hit_test_picks_closest_tick_under_slop() {
        let matches = vec![m(10), m(11)];
        let l = build_layout(800.0, 600.0, 100, &matches, 0, 0.0);
        let strip_x = l.x_dip + 4.0;
        // Click halfway between the two adjacent ticks — picks the closer.
        let mid =
            (l.ticks[0].y_dip + l.ticks[1].y_dip) * 0.5 + SEARCH_MINIMAP_TICK_HEIGHT_DIP * 0.5;
        let closer_to_first = mid - 0.5;
        let closer_to_second = mid + 0.5;
        assert_eq!(hit_test(&l, strip_x, closer_to_first, 10.0), Some(0));
        assert_eq!(hit_test(&l, strip_x, closer_to_second, 10.0), Some(1));
    }

    #[test]
    fn out_of_range_match_lines_clamp_to_buffer_end() {
        // A stale match that points past total_lines must still map to
        // a valid tick at the bottom of the strip rather than overflow.
        let matches = vec![m(999)];
        let l = build_layout(800.0, 600.0, 100, &matches, 0, 0.0);
        assert_eq!(l.ticks.len(), 1);
        assert!(l.ticks[0].y_dip <= 600.0);
        assert!(l.ticks[0].y_dip > 590.0);
    }
}
