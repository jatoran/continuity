//! Scroll extent of the focused view: the scroll-past-end overscroll
//! allowance and the largest admissible scroll offset.
//!
//! The wheel, the scrollbar, and every `ViewState::jump_to` clamp admit an
//! overscroll allowance below the document bottom when
//! `[editor].scroll_past_end` is on. Scroll *corrections* the user did not
//! ask for — the per-paint geometry anchor, the caret visibility floor, the
//! reflow anchor — must cap against the same ceiling. Capping them at the
//! classic `content_h - viewport_h` bottom instead is what yanked a view
//! parked in the overscroll zone back down on the first keystroke.
//!
//! Thread ownership: UI-thread-only; reads `Window::view_options` and the
//! focused `ViewState`.

use crate::Window;

impl Window {
    /// Scroll-past-end overscroll allowance (DIPs) for the focused view.
    ///
    /// When `[editor].scroll_past_end` is on this is `viewport_height -
    /// effective_line_height` so the last line can reach the viewport top;
    /// otherwise `0`. The value is zoom-correct because it derives from
    /// [`Self::effective_line_height`], which folds in the per-pane zoom.
    /// [`continuity_layout::ViewState`]'s clamp re-caps it and gates it to
    /// `0` when the content already fits, so it is always safe to set.
    #[must_use]
    pub(crate) fn overscroll_bottom_dip(&self) -> f32 {
        compute_overscroll_bottom_dip(
            self.view_options.scroll_past_end,
            self.surface.view.viewport_height_dip,
            self.effective_line_height(),
        )
    }

    /// Largest scroll offset the focused view admits for
    /// `content_height_dip`, overscroll allowance included. Derived from the
    /// live settings and geometry on every call rather than from the
    /// allowance cached on `ViewState`, so a correction can never be capped
    /// by a stale value.
    #[must_use]
    pub(crate) fn max_scroll_dip(&self, content_height_dip: f32) -> f32 {
        continuity_layout::compute_max_scroll_dip(
            content_height_dip,
            self.surface.view.viewport_height_dip,
            self.overscroll_bottom_dip(),
            self.effective_line_height(),
        )
    }

    /// Re-derive the overscroll allowance and row stride cached on the
    /// focused `ViewState` from the live settings, so `jump_to` /
    /// `scroll_instant` clamp against the current scroll extent.
    pub(crate) fn sync_view_scroll_extent(&mut self) {
        self.surface.view.line_height_dip = self.effective_line_height();
        self.surface.view.overscroll_bottom_dip = self.overscroll_bottom_dip();
    }
}

/// Pure overscroll-allowance computation shared by the focused-view helper
/// and the spectator-pane path (which works against a borrowed pane rather
/// than `self.surface.view`). Returns `viewport_height - line_height`
/// (clamped to `0`) when scroll-past-end is enabled, else `0`.
#[must_use]
pub(crate) fn compute_overscroll_bottom_dip(
    scroll_past_end: bool,
    viewport_height_dip: f32,
    line_height_dip: f32,
) -> f32 {
    if scroll_past_end {
        (viewport_height_dip - line_height_dip).max(0.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::compute_overscroll_bottom_dip;

    #[test]
    fn overscroll_allowance_lets_last_line_reach_viewport_top() {
        assert_eq!(compute_overscroll_bottom_dip(true, 700.0, 22.0), 678.0);
        assert_eq!(compute_overscroll_bottom_dip(false, 700.0, 22.0), 0.0);
        assert_eq!(compute_overscroll_bottom_dip(true, 10.0, 22.0), 0.0);
    }
}
