//! Mouse interaction baseline for the editor body.
//!
//! Click → place caret. Double-click → select word. Triple-click → select
//! line. Shift+click → extend selection. Drag → continuous selection from
//! the down-position to the current pointer.
//!
//! The triple-click detector is a small state machine that counts
//! consecutive clicks within `TRIPLE_CLICK_WINDOW_MS` of each other on the
//! same logical line.

const TRIPLE_CLICK_WINDOW_MS: u64 = 500;

use crate::pane_tree::{PaneId, SplitAxis, TabId};

/// Per-window mouse state: drag flag and triple-click history.
#[derive(Debug, Default)]
pub(crate) struct MouseState {
    /// `true` while the left button is held down.
    pub dragging: bool,
    /// Number of consecutive clicks on the same line within the time window.
    pub click_count: u32,
    /// Wall-clock millis at which the last click was registered.
    pub last_click_ms: u64,
    /// Logical line that was clicked on the most recent down event.
    pub last_click_line: i32,
    /// Active tab-strip drag, if any.
    pub tab_drag: Option<TabDrag>,
    /// Active pane-splitter drag (D3), if any.
    pub splitter_drag: Option<SplitterDrag>,
    /// Active vertical-scrollbar thumb drag, if any. Set on
    /// `WM_LBUTTONDOWN` over the thumb (and `SetCapture` called);
    /// cleared on `WM_LBUTTONUP` (and `ReleaseCapture` called). While
    /// set, `on_mouse_move` converts the pointer y into a scroll
    /// offset by inverting the current thumb geometry.
    pub scrollbar_drag: Option<ScrollbarDrag>,
    /// Active visual-pipe-table column-resize drag (Phase F), if any.
    /// Set on `WM_LBUTTONDOWN` over a column boundary (and `SetCapture`
    /// called); cleared on `WM_LBUTTONUP` (and `ReleaseCapture` +
    /// directive commit). While set, `on_mouse_move` updates the live
    /// width and the focused table layout previews at that width.
    pub table_col_drag: Option<TableColDrag>,
    /// `true` while the left button is dragging over the scaled-text
    /// minimap. Owned and mutated by this window's UI thread.
    pub minimap_dragging: bool,
    /// Pane whose body started the current text-selection drag.
    /// Autoscroll is allowed only while this remains the focused pane.
    pub selection_drag_pane: Option<PaneId>,
    /// Active vertical autoscroll for text-selection drag past the
    /// focused body edge. Owned and mutated only by this window's UI
    /// thread.
    pub autoscroll: Option<Autoscroll>,
    /// In-flight tab hover (D6). Set on every `WM_MOUSEMOVE` while the
    /// cursor is over a tab; cleared on mouse-out, palette open, or
    /// `Esc`. `None` outside the hover window.
    pub tab_hover: Option<crate::tab_hover::TabHover>,
    /// In-flight footnote hover-peek. Owned and mutated only by this
    /// window's UI thread; definition text is read from snapshots.
    pub footnote_hover: Option<crate::footnote_hover::FootnoteHover>,
    /// `true` after the UI thread arms Win32 `TME_LEAVE` tracking for
    /// this HWND. Cleared when `WM_MOUSELEAVE` arrives.
    pub mouse_leave_tracking: bool,
    /// `true` while the cursor sits inside the focused pane's line-number
    /// gutter strip. The renderer uses this to expand the gutter from
    /// caret-line-only to full line numbers on hover.
    pub gutter_hovered: bool,
    /// Source/display row currently under the cursor in the focused pane.
    pub line_hover: Option<MouseLineHover>,
    /// Live foreign-window tab-drag hover, broadcast from a sibling
    /// Continuity window currently dragging a tab over *this* window.
    /// When `Some`, paint draws the insertion-bar affordance on this
    /// window's tab strip so the user can see where a release will
    /// land. Cleared by an explicit "leave" broadcast or when the
    /// foreign source window closes.
    pub foreign_tab_drag_hover: Option<ForeignTabDragHover>,
    /// In-flight code-block copy-button hover. Owned and mutated only
    /// by this window's UI thread; set when the cursor sits over a
    /// rendered fenced code block whose caret is outside the block,
    /// cleared on mouse-out or caret-entry.
    pub code_copy_hover: Option<CodeCopyHover>,
}

/// Hovered source line plus exact display row under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MouseLineHover {
    /// Source line resolved through the last painted display row index.
    pub source_line: u32,
    /// Absolute display row under the cursor.
    pub display_row: u32,
    /// `true` when the cursor is inside this pane's gutter strip.
    pub in_gutter: bool,
}

/// Visible state of a fenced-block copy button under the cursor.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum CodeCopyFeedback {
    /// No recent copy — paint the idle/hover button.
    None,
    /// Recent successful copy — paint the success-tinted "Copied" label.
    Copied,
    /// Recent failed copy — paint the error-tinted "Failed" label.
    Failed,
}

/// Which kind of code surface this hover targets — drives the button's
/// geometry and the `event:code_copy kind=` payload.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum CodeCopyKind {
    /// `` ``` … ``` `` fenced block. Button sits at the painted
    /// block's top-right; `block_*_byte` covers the full fence
    /// including ticks; `inner_*_byte` is unused (the hit-test path
    /// re-derives inner content from the rope).
    Fenced,
    /// `` `code` `` inline run. Button sits above-right of the span's
    /// painted rect (offset upward so it doesn't crowd the line).
    /// `inner_*_byte` is the no-backticks range — the clipboard
    /// content for this hover; `block_*_byte` is the outer run with
    /// delimiters.
    Inline,
}

/// Live copy-button hover for a single code surface — fenced block or
/// inline span. Cached rects come from the painter (fenced) or the
/// renderer's `last_inline_code_hits` ring (inline) so paint and
/// hit-test see the exact same pixels even if the user keeps the
/// cursor still while the document reflows underneath.
#[derive(Clone, Debug)]
pub(crate) struct CodeCopyHover {
    /// Whether this hover targets a fenced block or an inline span.
    pub kind: CodeCopyKind,
    /// Source byte range of the outer code surface (fence ticks /
    /// backticks included). Used by the click-time clipboard read.
    pub block_start_byte: usize,
    /// Exclusive end byte of the outer surface.
    pub block_end_byte: usize,
    /// Inner-content byte range — what the click writes to the
    /// clipboard for inline spans. Equals `(block_start_byte,
    /// block_end_byte)` for fenced blocks (the fenced inner text is
    /// stripped at copy time from the full block).
    pub inner_start_byte: usize,
    /// Exclusive end byte of the inner content.
    pub inner_end_byte: usize,
    /// Button rect in client DIPs `(x, y, w, h)`.
    pub button_rect: (f32, f32, f32, f32),
    /// Whether the cursor currently sits inside `button_rect`.
    pub button_hovered: bool,
    /// Cached inner content (no fence ticks, no backticks). Refreshed
    /// on every move; the hit-test path re-reads from the rope so a
    /// stale cache cannot corrupt the clipboard.
    pub inner_text: String,
    /// Current feedback state. `Copied` / `Failed` revert to `None`
    /// when the feedback timer fires.
    pub feedback: CodeCopyFeedback,
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

/// Vertical-scrollbar thumb drag state.
///
/// The drag keeps the pressed offset inside the thumb instead of a
/// cached track slope. Each move inverts the current scrollbar layout
/// so row-index refinements cannot make the painted thumb and hit
/// rectangle use different ratios.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ScrollbarDrag {
    /// Pointer offset from the visible thumb top at button-down.
    pub thumb_grab_offset_dip: f32,
    /// Last pointer y seen during the drag, in client DIPs.
    pub last_mouse_y_dip: f32,
    /// Count of processed move samples during this drag.
    pub move_count: u32,
}

/// Phase F — live column-resize drag for a visual pipe-table. The drag
/// previews the new width every frame (the focused table layout is
/// rebuilt with a transient `TableColWidthOverride`) and commits the
/// final width to the table's `<!--continuity:width=…-->` directive on
/// release.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TableColDrag {
    /// Identifies the table by source `block_range.start`.
    pub block_start: usize,
    /// Column being resized — the one to the LEFT of the dragged
    /// boundary.
    pub col: u32,
    /// Client x (DIPs) at button-down.
    pub start_client_x: f32,
    /// The column's width (DIPs) at button-down.
    pub start_width: f32,
    /// Live width (DIPs) as the drag tracks the cursor.
    pub current_width: f32,
}

/// Direction for vertical text-selection autoscroll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutoscrollDirection {
    /// Cursor is above the focused body; scroll toward document start.
    Up,
    /// Cursor is below the focused body; scroll toward document end.
    Down,
}

impl AutoscrollDirection {
    /// Stable trace spelling.
    pub(crate) fn as_trace_str(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
        }
    }
}

/// In-flight text-selection autoscroll state.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Autoscroll {
    /// Last cursor x in client DIPs.
    pub last_cursor_x: i32,
    /// Last cursor y in client DIPs.
    pub last_cursor_y: i32,
    /// Scroll direction at the last edge-distance sample.
    pub direction: AutoscrollDirection,
    /// Positive distance past the body edge in DIPs.
    pub distance_dip: i32,
    /// Wall-clock millis when this autoscroll run started.
    pub started_ms: u64,
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

impl MouseState {
    /// Register a left-button-down event at wall-clock `now_ms` on `line`.
    /// Returns the resulting click count (1 = single, 2 = double, 3 = triple).
    pub(crate) fn register_click(&mut self, now_ms: u64, line: i32) -> u32 {
        if now_ms.saturating_sub(self.last_click_ms) <= TRIPLE_CLICK_WINDOW_MS
            && self.last_click_line == line
        {
            self.click_count = (self.click_count + 1).min(3);
        } else {
            self.click_count = 1;
        }
        self.last_click_ms = now_ms;
        self.last_click_line = line;
        self.dragging = true;
        self.click_count
    }
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

    #[test]
    fn click_count_increments_within_window() {
        let mut s = MouseState::default();
        assert_eq!(s.register_click(100, 4), 1);
        assert_eq!(s.register_click(200, 4), 2);
        assert_eq!(s.register_click(300, 4), 3);
        assert_eq!(s.register_click(900, 4), 1);
    }

    #[test]
    fn click_on_different_line_resets_count() {
        let mut s = MouseState::default();
        s.register_click(100, 4);
        assert_eq!(s.register_click(150, 5), 1);
    }
}
