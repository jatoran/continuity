//! Per-frame builder for [`continuity_render::TabDragOverlayDraw`].
//!
//! Run from [`crate::window_paint_builders::build_pane_chrome`] once per
//! paint to translate the UI-side `TabDrag` snapshot into the
//! renderer-facing payload that drives the in-flight drag visual
//! feedback (insertion bar, source-tab fade, pane-body highlight,
//! source-tab fade, and pane-body highlight).
//!
//! Geometry is recomputed in lock-step with `paint_pane_chrome` so the
//! painted bar lands exactly between two existing tabs the renderer is
//! about to draw — same width algorithm, same strip rect.
//!
//! Thread ownership: the owning window's UI thread.

use continuity_render::{
    pane_chrome, tab_slot_widths, PaneBodyDropHighlight, TabDragOverlayDraw, TabDragSourceFade,
    TabStripInsertionBarDraw,
};

use crate::mouse::{DropIndicator, TabDrag, TabDropResolution};
use crate::pane_layout::metrics;
use crate::window::Window;

/// Final fade-target alpha for the source tab. Picked so the label
/// reads as "lifted" without becoming illegible at low strip heights.
const SOURCE_TAB_FADE_ALPHA: f32 = 0.6;
/// Fade-in / fade-out duration in ms. Spec value per task body.
const TAB_DRAG_FADE_MS: u64 = 120;

/// Build the per-frame in-flight tab-drag affordance payload.
///
/// Returns `None` when no drag is in flight on this window *and* no
/// foreign window is broadcasting a drag-hover over this window's
/// strip — i.e. nothing to paint.
pub(crate) fn build_tab_drag_overlay(window: &Window) -> Option<TabDragOverlayDraw> {
    if let Some(local) = build_local_drag_overlay(window) {
        return Some(local);
    }
    build_foreign_drag_overlay(window)
}

fn build_local_drag_overlay(window: &Window) -> Option<TabDragOverlayDraw> {
    let drag = window.mouse_state.tab_drag.as_ref()?;
    let fade_alpha = compute_fade_alpha(window, drag.start_ms);
    let source_tab = compute_source_tab_fade(window, drag);
    let (indicator, pane_body, ghost) = match drag.resolution {
        TabDropResolution::Cancel => (None, None, None),
        TabDropResolution::SourceStrip(indicator) => {
            (compute_insertion_bar(window, indicator), None, None)
        }
        TabDropResolution::PaneBody { rect, .. } => {
            (None, Some(PaneBodyDropHighlight { body_rect: rect }), None)
        }
        // The cursor-attached tab is a screen-space helper window so it
        // stays visible outside this HWND. The render overlay owns only
        // in-window target affordances.
        TabDropResolution::ForeignWindow { .. } => (None, None, None),
        TabDropResolution::TearOff => (None, None, None),
    };
    if indicator.is_none() && source_tab.is_none() && pane_body.is_none() && ghost.is_none() {
        return None;
    }
    Some(TabDragOverlayDraw {
        source_strip_indicator: indicator,
        source_tab,
        pane_body_highlight: pane_body,
        ghost,
        fade_alpha,
    })
}

fn build_foreign_drag_overlay(window: &Window) -> Option<TabDragOverlayDraw> {
    let hover = window.mouse_state.foreign_tab_drag_hover?;
    let x = hover.cursor_x_dip.round() as i32;
    let y = hover.cursor_y_dip.round() as i32;
    let classification = classify_foreign_cursor(window, x, y);
    match classification {
        ForeignDropTarget::Strip(indicator) => {
            let bar = compute_insertion_bar(window, indicator)?;
            Some(TabDragOverlayDraw {
                source_strip_indicator: Some(bar),
                source_tab: None,
                pane_body_highlight: None,
                ghost: None,
                fade_alpha: 1.0,
            })
        }
        ForeignDropTarget::Body { rect } => Some(TabDragOverlayDraw {
            source_strip_indicator: None,
            source_tab: None,
            pane_body_highlight: Some(PaneBodyDropHighlight { body_rect: rect }),
            ghost: None,
            fade_alpha: 1.0,
        }),
        ForeignDropTarget::None => None,
    }
}

/// Classifier shared by the foreign-side painter and any future
/// preview/commit equivalence test. Pure aside from the `&Window`
/// reference, which is only used as a read-only view over the pane
/// tree + tab labels.
fn classify_foreign_cursor(window: &Window, x: i32, y: i32) -> ForeignDropTarget {
    if let Some(indicator) = window.compute_tab_drop_indicator(x, y) {
        return ForeignDropTarget::Strip(indicator);
    }
    let xf = x as f32;
    let yf = y as f32;
    let root = window.pane_root_rect();
    let inside = xf >= root.x && xf < root.x + root.w && yf >= root.y && yf < root.y + root.h;
    if inside {
        if let Some((_pane, rect)) = crate::pane_layout::pane_at_point(&window.tree, root, xf, yf) {
            if yf >= rect.y + metrics::TAB_STRIP_HEIGHT_DIP {
                return ForeignDropTarget::Body {
                    rect: (rect.x, rect.y, rect.w, rect.h),
                };
            }
        }
    }
    // Cursor is over the window's chrome (title bar, status bar, blank
    // space outside any leaf). Fall back to the focused pane so the
    // user still sees *something* — release will adopt into the
    // focused pane regardless of the chrome strip the cursor sits on.
    if let Some(rect) = window.pane_body_rect(window.tree.focused) {
        return ForeignDropTarget::Body {
            rect: (rect.x, rect.y, rect.w, rect.h),
        };
    }
    ForeignDropTarget::None
}

/// Foreign-side drop affordance classification — what should the
/// painter draw when this window receives a sibling's drag hover at
/// `(x, y)` in its own client DIPs?
#[derive(Debug, Clone, Copy)]
enum ForeignDropTarget {
    /// Cursor over this window's tab strip ⇒ paint insertion bar at
    /// the indicator slot.
    Strip(DropIndicator),
    /// Cursor over a pane body in this window ⇒ paint the body
    /// highlight on that pane.
    Body {
        /// Body rect in client DIPs.
        rect: (f32, f32, f32, f32),
    },
    /// No affordance — pane tree empty (window not fully initialized).
    None,
}

/// 120 ms ease-out-cubic fade-in for the in-flight affordance.
/// Reduced motion collapses to 1.0 so accessibility users still see the
/// feedback (per task body: affordances are not hidden under reduced
/// motion, only their fades are).
fn compute_fade_alpha(window: &Window, start_ms: u64) -> f32 {
    if window.motion_policy.is_reduced_motion() {
        return 1.0;
    }
    let now = crate::window_mouse_hover::wall_clock_ms();
    let elapsed = now.saturating_sub(start_ms);
    if elapsed >= TAB_DRAG_FADE_MS {
        return 1.0;
    }
    let t = elapsed as f32 / TAB_DRAG_FADE_MS as f32;
    // Cubic ease-out: 1 - (1 - t)^3
    let u = 1.0 - t;
    1.0 - u * u * u
}

fn compute_source_tab_fade(window: &Window, drag: &TabDrag) -> Option<TabDragSourceFade> {
    let leaves = window.pane_outer_rects();
    let mut pane_iter = leaves
        .iter()
        .enumerate()
        .filter(|(_, (id, _))| *id == drag.pane);
    let (_, (_, source_rect)) = pane_iter.next()?;
    let group = window.tree.groups.get(&drag.pane)?;
    let tab_index = group.tabs.iter().position(|t| *t == drag.tab)?;
    let alpha = SOURCE_TAB_FADE_ALPHA;
    Some(TabDragSourceFade {
        strip_outer: (source_rect.x, source_rect.y, source_rect.w, source_rect.h),
        tab_index,
        alpha,
    })
}

fn compute_insertion_bar(
    window: &Window,
    indicator: DropIndicator,
) -> Option<TabStripInsertionBarDraw> {
    let leaves = window.pane_outer_rects();
    let (_, outer) = leaves.iter().find(|(id, _)| *id == indicator.pane)?;
    let group = window.tree.groups.get(&indicator.pane)?;
    let labels: Vec<String> = group
        .tabs
        .iter()
        .map(|tid| {
            window
                .tree
                .tabs
                .get(tid)
                .map(|t| window.tab_label(t))
                .unwrap_or_default()
        })
        .collect();
    let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let widths = tab_slot_widths(&label_refs, outer.w);
    let x_in_strip = insertion_bar_x_in_strip(&widths, indicator.slot, outer.w);
    Some(TabStripInsertionBarDraw {
        strip_outer: (outer.x, outer.y, outer.w, outer.h),
        x_in_strip,
        width: pane_chrome::BORDER_ACTIVE_DIP, // 2 DIP — same metric the focused border uses.
        height: metrics::TAB_STRIP_HEIGHT_DIP,
    })
}

/// Strip-relative x where the insertion bar should be painted for
/// slot `slot`. `slot == 0` ⇒ before tab 0. `slot == widths.len()` ⇒
/// after the last tab. The bar straddles the slot boundary so the
/// renderer paints it 1 DIP inset on either side of the gap.
#[must_use]
pub(crate) fn insertion_bar_x_in_strip(widths: &[f32], slot: usize, strip_w: f32) -> f32 {
    if widths.is_empty() {
        return 0.0;
    }
    let slot = slot.min(widths.len());
    let acc: f32 = widths.iter().take(slot).sum();
    // Pin past-rightmost bars to the strip's inner edge so a 2 DIP bar
    // stays fully visible even when the last tab consumes the entire
    // strip width.
    let max_x = (strip_w - pane_chrome::BORDER_ACTIVE_DIP).max(0.0);
    acc.min(max_x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insertion_bar_starts_at_zero_for_slot_zero() {
        let widths = [120.0, 120.0, 120.0];
        assert_eq!(insertion_bar_x_in_strip(&widths, 0, 400.0), 0.0);
    }

    #[test]
    fn insertion_bar_between_slots_lands_at_boundary() {
        let widths = [120.0, 120.0, 120.0];
        assert!((insertion_bar_x_in_strip(&widths, 1, 400.0) - 120.0).abs() < 0.01);
        assert!((insertion_bar_x_in_strip(&widths, 2, 400.0) - 240.0).abs() < 0.01);
    }

    #[test]
    fn insertion_bar_past_last_clamps_to_strip_minus_bar_width() {
        let widths = [120.0, 120.0, 120.0];
        // After last tab = 360, but strip_w = 400 ⇒ allowed (< 400 - 2).
        assert!((insertion_bar_x_in_strip(&widths, 3, 400.0) - 360.0).abs() < 0.01);
        // After last tab = 400 but strip_w = 400 ⇒ clamped to 398.
        let tight = [200.0, 200.0];
        assert!((insertion_bar_x_in_strip(&tight, 2, 400.0) - 398.0).abs() < 0.01);
    }

    #[test]
    fn empty_widths_returns_zero_for_any_slot() {
        let widths: [f32; 0] = [];
        assert_eq!(insertion_bar_x_in_strip(&widths, 0, 100.0), 0.0);
        assert_eq!(insertion_bar_x_in_strip(&widths, 5, 100.0), 0.0);
    }

    /// Symmetry check from the task body: the bar between tab N and
    /// N+1 must correspond to dropping at slot N+1 — `tab_drop_slot`
    /// agrees with `insertion_bar_x_in_strip` on the slot boundary.
    #[test]
    fn slot_boundary_round_trips_through_drop_slot() {
        use crate::window_mouse_tabs::tab_drop_slot;
        let widths = [100.0, 100.0, 100.0];
        for slot in 1..widths.len() {
            let x = insertion_bar_x_in_strip(&widths, slot, 400.0);
            // The bar's left edge is on the boundary; nudge a hair to
            // the right so the midpoint test routes us *into* the
            // following tab and back to `slot`.
            let resolved = tab_drop_slot(&widths, x + 0.01);
            assert_eq!(resolved, slot, "slot {} round-trip failed", slot);
        }
    }
}
