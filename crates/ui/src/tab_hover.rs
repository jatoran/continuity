//! Phase D6 — tab hover preview state machine.
//!
//! A buffer-content popover appears ~400 ms after the cursor settles on
//! a tab in the tab strip and is dismissed by mouse-out, palette open,
//! or `Esc`. This module owns the *state* and the decision oracle; the
//! popover paint is consumed by the renderer in a follow-up pass.
//!
//! State machine:
//!
//! ```text
//! cursor enters tab N at t₀
//!     ↓
//!   TabHover { tab: N, started_ms: t₀ }
//!     ↓ (t - t₀ >= TAB_HOVER_PREVIEW_MS)
//!   should_show_preview → true   (renderer paints popover)
//!     ↓ (mouseout / palette open / Esc)
//!   cleared back to None
//! ```
//!
//! Different-tab moves restart the timer; same-tab moves are no-ops so
//! the popover doesn't flicker as the user wiggles inside one tab.

use crate::pane_tree::{PaneId, TabId};

/// Hover dwell required before the preview materializes. Spec D6 names
/// ~400 ms; the renderer reads this constant via `TabHover::elapsed_ms`.
pub const TAB_HOVER_PREVIEW_MS: u64 = 400;

/// In-flight tab hover state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TabHover {
    /// Pane whose strip the cursor is over.
    pub pane: PaneId,
    /// Tab the cursor is over (a member of `pane`'s `tabs` vector).
    pub tab: TabId,
    /// Wall-clock millis when the hover started (cursor first entered
    /// this tab).
    pub started_ms: u64,
}

impl TabHover {
    /// Construct a fresh hover record at `now_ms`.
    #[must_use]
    pub fn new(pane: PaneId, tab: TabId, now_ms: u64) -> Self {
        Self {
            pane,
            tab,
            started_ms: now_ms,
        }
    }

    /// Elapsed millis since `started_ms`. Saturating subtraction guards
    /// against a non-monotonic clock so the preview can't disappear and
    /// re-appear on a backwards tick.
    #[must_use]
    pub fn elapsed_ms(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.started_ms)
    }
}

/// Decision oracle — `true` once the hover has dwelled long enough to
/// materialize the popover. Used by the renderer's per-frame check.
#[must_use]
pub fn should_show_preview(hover: &TabHover, now_ms: u64, threshold_ms: u64) -> bool {
    hover.elapsed_ms(now_ms) >= threshold_ms
}

/// Decide whether a tab's close affordance is visible for the current
/// close-button mode and UI-thread hover slot.
#[must_use]
pub(crate) fn is_tab_close_visible(
    mode: continuity_config::TabCloseButton,
    hover: Option<TabHover>,
    pane: PaneId,
    tab: TabId,
) -> bool {
    match mode {
        continuity_config::TabCloseButton::Always => true,
        continuity_config::TabCloseButton::Hover => {
            hover.is_some_and(|hover| hover.pane == pane && hover.tab == tab)
        }
        continuity_config::TabCloseButton::Never => false,
    }
}

/// Update an existing hover slot from a fresh mouse position.
///
/// Inputs:
/// - `slot`: the current `Option<TabHover>` (passed `&mut`).
/// - `over`: `Some((pane, tab))` when the cursor is over a tab strip
///   entry, else `None`.
/// - `now_ms`: wall-clock millis from the caller's clock.
///
/// Returns `true` when the slot's contents actually changed — callers
/// can use this to gate `request_invalidate`. Behavior:
/// - cursor over a *different* tab than the slot already holds →
///   start a fresh hover record (timer resets).
/// - cursor over the *same* tab → no-op (preserve `started_ms`).
/// - cursor over no tab → clear slot.
pub(crate) fn update_tab_hover(
    slot: &mut Option<TabHover>,
    over: Option<(PaneId, TabId)>,
    now_ms: u64,
) -> bool {
    match (slot.as_ref(), over) {
        (None, None) => false,
        (None, Some((pane, tab))) => {
            *slot = Some(TabHover::new(pane, tab, now_ms));
            true
        }
        (Some(h), None) => {
            let _ = h;
            *slot = None;
            true
        }
        (Some(h), Some((pane, tab))) => {
            if h.pane == pane && h.tab == tab {
                false
            } else {
                *slot = Some(TabHover::new(pane, tab, now_ms));
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_pair() -> (PaneId, TabId) {
        (PaneId::fresh(), TabId::fresh())
    }

    #[test]
    fn elapsed_ms_returns_zero_at_start() {
        let (p, t) = fresh_pair();
        let h = TabHover::new(p, t, 1000);
        assert_eq!(h.elapsed_ms(1000), 0);
    }

    #[test]
    fn elapsed_ms_grows_with_time() {
        let (p, t) = fresh_pair();
        let h = TabHover::new(p, t, 1000);
        assert_eq!(h.elapsed_ms(1500), 500);
    }

    #[test]
    fn elapsed_ms_saturates_on_clock_jitter() {
        let (p, t) = fresh_pair();
        let h = TabHover::new(p, t, 1000);
        // Earlier `now` (backwards clock) → 0, not negative.
        assert_eq!(h.elapsed_ms(500), 0);
    }

    #[test]
    fn should_show_preview_waits_for_threshold() {
        let (p, t) = fresh_pair();
        let h = TabHover::new(p, t, 1000);
        assert!(!should_show_preview(&h, 1399, TAB_HOVER_PREVIEW_MS));
        assert!(should_show_preview(&h, 1400, TAB_HOVER_PREVIEW_MS));
        assert!(should_show_preview(&h, 9999, TAB_HOVER_PREVIEW_MS));
    }

    #[test]
    fn hover_mode_shows_close_only_for_hovered_tab() {
        let (pane, hovered_tab) = fresh_pair();
        let other_tab = TabId::fresh();
        let hover = Some(TabHover::new(pane, hovered_tab, 1000));

        assert!(is_tab_close_visible(
            continuity_config::TabCloseButton::Hover,
            hover,
            pane,
            hovered_tab
        ));
        assert!(!is_tab_close_visible(
            continuity_config::TabCloseButton::Hover,
            hover,
            pane,
            other_tab
        ));
    }

    #[test]
    fn hover_mode_hides_close_when_no_tab_hovered() {
        let (pane, tab) = fresh_pair();

        assert!(!is_tab_close_visible(
            continuity_config::TabCloseButton::Hover,
            None,
            pane,
            tab
        ));
    }

    #[test]
    fn explicit_close_modes_ignore_hover_target() {
        let (pane, tab) = fresh_pair();

        assert!(is_tab_close_visible(
            continuity_config::TabCloseButton::Always,
            None,
            pane,
            tab
        ));
        assert!(!is_tab_close_visible(
            continuity_config::TabCloseButton::Never,
            Some(TabHover::new(pane, tab, 1000)),
            pane,
            tab
        ));
    }

    #[test]
    fn update_from_none_to_tab_starts_hover() {
        let mut slot: Option<TabHover> = None;
        let (p, t) = fresh_pair();
        let changed = update_tab_hover(&mut slot, Some((p, t)), 1000);
        assert!(changed);
        let h = slot.expect("hover set");
        assert_eq!(h.started_ms, 1000);
        assert_eq!(h.tab, t);
    }

    #[test]
    fn update_same_tab_is_noop_preserves_started_ms() {
        let (p, t) = fresh_pair();
        let mut slot = Some(TabHover::new(p, t, 1000));
        let changed = update_tab_hover(&mut slot, Some((p, t)), 2000);
        assert!(!changed);
        assert_eq!(slot.unwrap().started_ms, 1000);
    }

    #[test]
    fn update_different_tab_restarts_timer() {
        let (p, t1) = fresh_pair();
        let t2 = TabId::fresh();
        let mut slot = Some(TabHover::new(p, t1, 1000));
        let changed = update_tab_hover(&mut slot, Some((p, t2)), 2000);
        assert!(changed);
        let h = slot.unwrap();
        assert_eq!(h.tab, t2);
        assert_eq!(h.started_ms, 2000);
    }

    #[test]
    fn update_to_none_clears_slot() {
        let (p, t) = fresh_pair();
        let mut slot = Some(TabHover::new(p, t, 1000));
        let changed = update_tab_hover(&mut slot, None, 2000);
        assert!(changed);
        assert!(slot.is_none());
    }

    #[test]
    fn update_already_clear_is_noop() {
        let mut slot: Option<TabHover> = None;
        let changed = update_tab_hover(&mut slot, None, 1000);
        assert!(!changed);
        assert!(slot.is_none());
    }
}
