//! Anchor payload + pure scroll-restoration math for
//! [`crate::window_caret_anchor`].
//!
//! [`CaretAnchor`] is the value captured before a reflow and consumed
//! after it; [`AnchorTarget`] names which line it holds. The two
//! `anchored_scroll*` functions are `Window`-free so the restore contract
//! is unit-testable (`window_caret_anchor/tests.rs`).
//!
//! Thread ownership: values only; the owning `Window` runs on its UI
//! thread.

use continuity_text::Position;

/// Which line the anchor holds still across a reflow.
///
/// The principle is "the line the user is looking at keeps its screen y".
/// When the caret is inside the viewport that is the caret line. When the
/// user has scrolled the caret off screen (reading elsewhere in the
/// document) the caret line's y is meaningless to them, so the source line
/// at the viewport's top edge is held instead — and the caret is never
/// pulled back into view by a reflow it did not ask for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AnchorTarget {
    /// The primary caret's display row overlapped the viewport.
    VisibleCaret,
    /// The caret was off screen; the source line at the viewport's top
    /// edge is held at its (non-positive) screen y.
    ViewportTopLine,
    /// The caret was off screen and no projection could name the top
    /// line; the caret line's off-screen y is held without any clamp
    /// into the viewport.
    OffScreenCaret,
}

/// Captured anchor state prior to a reflow.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CaretAnchor {
    /// Source-rope position of the anchored line. For
    /// [`AnchorTarget::VisibleCaret`] / [`AnchorTarget::OffScreenCaret`]
    /// this is the primary caret head; for
    /// [`AnchorTarget::ViewportTopLine`] it is byte 0 of the source line
    /// at the viewport top. The bytes do not move during a pure reflow,
    /// but the display line they map to may change (wrap, fold, scale).
    pub(super) position: Position,
    /// Screen y (pane-body-relative) of the anchored display line at the
    /// moment the snapshot was taken. This is the value we want to
    /// restore after the closure runs.
    pub(super) screen_y: f32,
    /// What `position` denotes and which restore rule applies.
    pub(super) target: AnchorTarget,
}

/// `true` when a display row whose top sits at `screen_y` (pane-body
/// DIPs) overlaps a viewport of height `viewport_h`. A zero-height
/// viewport (view not yet laid out) counts as overlapping so the legacy
/// caret hold applies until geometry is known.
#[must_use]
pub(crate) fn is_row_overlapping_viewport(
    screen_y: f32,
    line_height: f32,
    viewport_h: f32,
) -> bool {
    if viewport_h <= 0.0 {
        return true;
    }
    screen_y + line_height > 0.0 && screen_y < viewport_h
}

/// Pure scroll-restoration math for a caret line that was on screen.
/// Given the caret's new line top, the desired pre-reflow screen y, and
/// the post-reflow viewport/content heights, returns the scroll position
/// that places the caret line at `screen_y_before` — clamped into
/// `[0, max_scroll]` and into the viewport.
#[must_use]
pub(crate) fn anchored_scroll(
    new_line_top: f32,
    line_height: f32,
    screen_y_before: f32,
    viewport_h: f32,
    content_h: f32,
) -> f32 {
    let max_scroll = (content_h - viewport_h).max(0.0);
    let target = (new_line_top - screen_y_before).clamp(0.0, max_scroll);
    let proposed_screen_y = new_line_top - target;
    // Caret-line would land below the viewport — pull scroll so the
    // caret bottom touches the viewport bottom instead. On-screen wins
    // over "right y" when the viewport shrunk past the target.
    if proposed_screen_y + line_height > viewport_h && viewport_h > 0.0 {
        return ((new_line_top + line_height - viewport_h).max(0.0)).min(max_scroll);
    }
    // Caret-line would land above the viewport — pin it to the top.
    if proposed_screen_y < 0.0 {
        return new_line_top.min(max_scroll);
    }
    target
}

/// Pure scroll-restoration math for an anchored line that was **not**
/// required to be on screen (the viewport-top line, or an off-screen
/// caret). Places the line at `screen_y_before` and clamps only into
/// `[0, max_scroll]`; there is deliberately no pull-into-viewport rule,
/// because that rule is what scrolled the user away from what they were
/// reading whenever a reflow fired with the caret off screen.
#[must_use]
pub(crate) fn anchored_scroll_without_reveal(
    new_line_top: f32,
    screen_y_before: f32,
    viewport_h: f32,
    content_h: f32,
) -> f32 {
    let max_scroll = (content_h - viewport_h).max(0.0);
    (new_line_top - screen_y_before).clamp(0.0, max_scroll)
}
