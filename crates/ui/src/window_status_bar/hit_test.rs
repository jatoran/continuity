//! Pure pixel hit-test against the cached status-bar segment layout.
//! Kept HWND-free so it's unit-testable without a window.

use continuity_render::{SegmentBounds, StatusBarSegmentKind, STATUS_BAR_HEIGHT_DIP};

/// Hit-test `(x_dip, y_dip)` against the cached layout and return the
/// matching segment kind. Pure helper so it's unit-testable without an
/// `HWND`.
#[must_use]
pub fn hit_test(
    bounds: &[SegmentBounds],
    top: f32,
    x_dip: f32,
    y_dip: f32,
) -> Option<StatusBarSegmentKind> {
    if y_dip < top || y_dip > top + STATUS_BAR_HEIGHT_DIP {
        return None;
    }
    bounds
        .iter()
        .find(|b| x_dip >= b.left && x_dip <= b.right)
        .map(|b| b.kind)
}

/// Return whether a status segment currently dispatches a click action.
#[must_use]
pub fn is_clickable(kind: StatusBarSegmentKind) -> bool {
    matches!(
        kind,
        StatusBarSegmentKind::Position
            | StatusBarSegmentKind::Chars
            | StatusBarSegmentKind::Words
            | StatusBarSegmentKind::Lines
            | StatusBarSegmentKind::LineEndings
            | StatusBarSegmentKind::Chip
            | StatusBarSegmentKind::VaultLauncher
            | StatusBarSegmentKind::VaultFiles
            | StatusBarSegmentKind::VaultMinimap
            | StatusBarSegmentKind::VaultOutline
            | StatusBarSegmentKind::VaultSettings
    )
}
