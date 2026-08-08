//! Mouse interaction baseline for the editor body.
//!
//! Click → place caret. Double-click → select word. Triple-click → select
//! line. Shift+click → extend selection. Drag → continuous selection from
//! the down-position to the current pointer.
//!
use crate::pane_tree::{PaneId, SplitAxis, TabId};

/// Desktop-shell mouse state for HWND capture and chrome interactions.
#[derive(Debug, Default)]
pub(crate) struct MouseState {
    /// `true` while the left button is held down.
    pub dragging: bool,
    /// Active tab-strip drag, if any.
    pub tab_drag: Option<TabDrag>,
    /// Active pane-splitter drag (D3), if any.
    pub splitter_drag: Option<SplitterDrag>,
    /// In-flight tab hover (D6). Set on every `WM_MOUSEMOVE` while the
    /// cursor is over a tab; cleared on mouse-out, palette open, or
    /// `Esc`. `None` outside the hover window.
    pub tab_hover: Option<crate::tab_hover::TabHover>,
    /// `true` after the UI thread arms Win32 `TME_LEAVE` tracking for
    /// this HWND. Cleared when `WM_MOUSELEAVE` arrives.
    pub mouse_leave_tracking: bool,
    /// Live foreign-window tab-drag hover, broadcast from a sibling
    /// Continuity window currently dragging a tab over *this* window.
    /// When `Some`, paint draws the insertion-bar affordance on this
    /// window's tab strip so the user can see where a release will
    /// land. Cleared by an explicit "leave" broadcast or when the
    /// foreign source window closes.
    pub foreign_tab_drag_hover: Option<ForeignTabDragHover>,
    /// Active outline-sidebar edge-resize drag, if any.
    pub outline_resize_drag: Option<OutlineResizeDrag>,
    /// Active left file-tree right-edge resize drag.
    pub file_tree_resize_drag: Option<FileTreeResizeDrag>,
    /// Click-or-drag candidate originating on a file-tree row.
    pub file_tree_entry_drag: Option<FileTreeEntryDrag>,
}

/// In-flight outline-sidebar width drag.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OutlineResizeDrag {
    /// Client x where the drag started.
    pub start_x: i32,
    /// Sidebar width (DIPs) when the drag started.
    pub start_width_dip: f32,
}

/// In-flight left file-tree width drag.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FileTreeResizeDrag {
    pub(crate) start_x: i32,
    pub(crate) start_width_dip: f32,
}

/// A file-tree row press that may become a contained move.
#[derive(Clone, Debug)]
pub(crate) struct FileTreeEntryDrag {
    pub(crate) relative: std::path::PathBuf,
    pub(crate) kind: continuity_render::FileTreeEntryKind,
    pub(crate) size_bytes: Option<u64>,
    pub(crate) disposition: crate::window_config::FileOpenDisposition,
    pub(crate) start_x: i32,
    pub(crate) start_y: i32,
    pub(crate) current_x: i32,
    pub(crate) current_y: i32,
    pub(crate) is_dragging: bool,
}

/// Pane-splitter drag state — captures the split's axis + an anchor leaf
/// in the left/top branch so `pane_layout::resize_focused` can target the
/// correct enclosing split, plus the root-rect dimension and starting
/// mouse position so deltas are computed in DIPs against the painted
/// frame the drag began on.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SplitterDrag {
    /// Axis of the split being resized.
    pub axis: SplitAxis,
    /// Any leaf id in the left (Horizontal) or top (Vertical) branch
    /// adjacent to the dragged splitter. Passing this to
    /// `pane_layout::nudge_ratio` resizes that specific split.
    pub left_leaf: PaneId,
    /// Starting client x at button-down.
    pub start_x: i32,
    /// Starting client y at button-down.
    pub start_y: i32,
    /// Root-rect width at drag start (used as the denominator for x-axis
    /// ratio deltas in Horizontal splits).
    pub root_w: f32,
    /// Root-rect height at drag start (used for Vertical splits).
    pub root_h: f32,
}

/// Tab-strip drag origin for mouse tear-off.
#[derive(Debug, Clone)]
pub(crate) struct TabDrag {
    /// Pane where the drag started.
    pub pane: PaneId,
    /// Tab where the drag started.
    pub tab: TabId,
    /// Cached tab label at drag start. Used as the ghost-preview text
    /// when the cursor sits in the tear-off zone; cheaper than reaching
    /// back into the buffer per WM_MOUSEMOVE for a string that does not
    /// change while the drag is in flight.
    pub label: String,
    /// Starting client x.
    pub start_x: i32,
    /// Starting client y.
    pub start_y: i32,
    /// Wall-clock ms when the drag began. Used by the `event:tab_drag`
    /// trace to record elapsed time at every resolution transition.
    pub start_ms: u64,
    /// Live drop indicator: the pane whose strip the cursor is currently
    /// over plus the insertion slot (0..=tabs.len()) the drop would land
    /// at. `None` when the cursor is outside any pane's tab strip — the
    /// painter then suppresses the indicator. Recomputed on every
    /// `WM_MOUSEMOVE` so the renderer can draw without re-running the
    /// hit-test.
    pub drop_indicator: Option<DropIndicator>,
    /// Live drop resolution mirror — the same answer
    /// [`super::window_mouse_tabs::compute_tab_drop_resolution`] would
    /// return at the current cursor position. Recomputed on every
    /// `WM_MOUSEMOVE` so the renderer and the trace log read the same
    /// resolution the next `WM_LBUTTONUP` will commit. The variant
    /// carries the data each affordance needs (target pane, foreign
    /// HWND, etc.).
    pub resolution: TabDropResolution,
    /// Current drag phase. Starts `Armed` on press, advances to
    /// `Reorder` once past the arm threshold, and `Detached` once the
    /// cursor is pulled vertically out of the strip band. Hysteretic, so
    /// it is owned here and mutated only on the `WM_MOUSEMOVE` path; the
    /// commit path reads it back so the painted affordance and the
    /// committed drop never diverge.
    pub phase: TabDragPhase,
}

/// Phase of an in-flight tab drag (Chrome-style two-stage grab).
///
/// `Armed` shows nothing — a press that releases here is a plain click.
/// `Reorder` is the grounded, in-strip phase: an insertion bar tracks
/// the cursor's slot but the tab is not lifted. `Detached` is the
/// floating phase reached by pulling the cursor vertically out of the
/// strip band; the screen-space ghost follows in 2D and the full drop
/// resolver (pane body / foreign window / tear-off) is live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TabDragPhase {
    /// Press registered; cursor still within the arm threshold.
    Armed,
    /// Past the arm threshold but still grounded in the strip band.
    Reorder,
    /// Lifted vertically out of the strip band — floating drag.
    Detached,
}

impl TabDragPhase {
    /// `true` only in the floating phase. Gates the screen-space ghost
    /// and every off-strip drop resolution.
    pub(crate) fn is_detached(self) -> bool {
        matches!(self, Self::Detached)
    }
}

/// Drop slot for an in-flight tab drag — pane + insertion index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DropIndicator {
    /// Pane whose tab strip is under the cursor.
    pub pane: PaneId,
    /// Insertion index into that pane's `tabs` vector (0..=len).
    pub slot: usize,
}

/// Resolution the next `WM_LBUTTONUP` would commit for an in-flight
/// tab drag. Recomputed on every `WM_MOUSEMOVE` so the live painted
/// affordance and the commit-time decision read the same answer.
#[derive(Debug, Clone, Copy)]
pub(crate) enum TabDropResolution {
    /// Cursor never left a hysteresis radius around the press point —
    /// release is a no-op (activation already happened on LBUTTONDOWN).
    Cancel,
    /// Cursor sits over a pane tab strip in *this* window. Drop reorders
    /// when `pane == drag.pane` or moves the tab when different.
    SourceStrip(DropIndicator),
    /// Cursor sits over a pane *body* (not the strip) in this window.
    /// Drop moves (or with Ctrl, clones) the tab into the target pane.
    PaneBody {
        /// Target pane id.
        pane: PaneId,
        /// Target pane body rect in client DIPs.
        rect: (f32, f32, f32, f32),
    },
    /// Cursor sits inside a sibling Continuity window's window rect.
    /// Drop adopts the tab into that window.
    ForeignWindow {
        /// Sibling HWND stored as raw `isize` so the resolution stays `Copy`.
        hwnd_raw: isize,
    },
    /// Anywhere else — desktop, another app, this window's chrome
    /// outside any pane. Drop tears off into a new Continuity window.
    TearOff,
}

impl TabDropResolution {
    /// Trace spelling — stable identifier per variant for the
    /// `event:tab_drag` log line.
    pub(crate) fn as_trace_str(&self) -> &'static str {
        match self {
            Self::Cancel => "cancel",
            Self::SourceStrip(_) => "source_strip",
            Self::PaneBody { .. } => "pane_body",
            Self::ForeignWindow { .. } => "foreign_window",
            Self::TearOff => "tear_off",
        }
    }
}

/// Cross-window broadcast payload: another Continuity window's drag
/// is currently hovering this window. Stored on the receiver so its
/// paint pass can draw the insertion bar on its tab strip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ForeignTabDragHover {
    /// Source window's HWND as raw `isize` — used to distinguish a
    /// stale hover from a fresh one when multiple windows are dragging.
    pub source_hwnd_raw: isize,
    /// Client-space cursor coordinates in *this* window's DIPs.
    pub cursor_x_dip: f32,
    /// Client-space cursor y in this window's DIPs.
    pub cursor_y_dip: f32,
}

/// Map a (client-area pixel y, line height in DIPs) to a 0-indexed buffer
/// line. Negative pixels clamp to line 0.
#[must_use]
pub fn pixel_y_to_line(y: i32, line_height: f32) -> u32 {
    if y <= 0 || line_height <= 0.0 {
        return 0;
    }
    ((y as f32 / line_height).floor() as i64).max(0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_y_zero_returns_first_line() {
        assert_eq!(pixel_y_to_line(0, 20.0), 0);
        assert_eq!(pixel_y_to_line(-5, 20.0), 0);
    }

    #[test]
    fn pixel_y_maps_to_line() {
        assert_eq!(pixel_y_to_line(10, 20.0), 0);
        assert_eq!(pixel_y_to_line(20, 20.0), 1);
        assert_eq!(pixel_y_to_line(45, 20.0), 2);
    }
}
