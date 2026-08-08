//! Portable editor-body pointer interaction state.
//!
//! HWND capture, cursor APIs, and desktop chrome drags remain in the Win32
//! shell. This module owns state whose lifetime follows one editor surface.

use continuity_host::{PointerButton, PointerIntent, PointerPhase};

use crate::pane_tree::PaneId;

const TRIPLE_CLICK_WINDOW_MS: u64 = 500;

/// Editor-body pointer, hover, and selection-drag state.
///
/// **Thread ownership:** the UI thread that owns the containing
/// [`super::EditorSurface`] is the sole writer.
#[derive(Debug, Default)]
pub(crate) struct PointerState {
    /// Number of consecutive clicks on the same line within the time window.
    pub(crate) click_count: u32,
    /// Wall-clock millis at which the last click was registered.
    last_click_ms: u64,
    /// Logical line that was clicked on the most recent down event.
    last_click_line: i32,
    /// Active vertical-scrollbar thumb drag, if any.
    pub(crate) scrollbar_drag: Option<ScrollbarDrag>,
    /// Active visual-pipe-table column-resize drag, if any.
    pub(crate) table_col_drag: Option<TableColumnDrag>,
    /// Whether the left button is dragging over the scaled-text minimap.
    pub(crate) minimap_dragging: bool,
    /// Pane whose body started the current text-selection drag.
    pub(crate) selection_drag_pane: Option<PaneId>,
    /// Active vertical autoscroll for a text-selection drag.
    pub(crate) autoscroll: Option<Autoscroll>,
    /// In-flight footnote hover-peek.
    pub(crate) footnote_hover: Option<crate::footnote_hover::FootnoteHover>,
    /// Whether the cursor sits inside the focused pane's line-number gutter.
    pub(crate) gutter_hovered: bool,
    /// Source/display row currently under the cursor in the focused pane.
    pub(crate) line_hover: Option<LineHover>,
    /// In-flight code-block copy-button hover.
    pub(crate) code_copy_hover: Option<CodeCopyHover>,
    /// Whether a Ctrl+drag is building an additional selection.
    pub(crate) multi_select_drag: bool,
    /// Whole-word selection captured by a double-click.
    pub(crate) word_drag_origin: Option<continuity_text::Selection>,
}

impl PointerState {
    /// Route one normalized host pointer intent to the surface action consumed
    /// by the current native adapter.
    pub(crate) fn route_intent(&mut self, intent: PointerIntent) -> PointerAction {
        let x = intent.x_dip.round() as i32;
        let y = intent.y_dip.round() as i32;
        let key_state = compute_native_key_state(intent);
        match (intent.phase, intent.button) {
            (PointerPhase::Down, PointerButton::Primary) if intent.click_count >= 2 => {
                PointerAction::PrimaryDoubleDown { x, y }
            }
            (PointerPhase::Down, PointerButton::Primary) => {
                PointerAction::PrimaryDown { x, y, key_state }
            }
            (PointerPhase::Up, PointerButton::Primary) => PointerAction::PrimaryUp { x, y },
            (PointerPhase::Down, PointerButton::Middle) => PointerAction::MiddleDown { x, y },
            (PointerPhase::Move, _) => PointerAction::Move { x, y, key_state },
            (PointerPhase::Leave, _) => PointerAction::Leave,
            (PointerPhase::Cancel, _) => PointerAction::Cancel,
            _ => PointerAction::Ignored,
        }
    }

    /// Register a left-button-down event at wall-clock `now_ms` on `line`.
    ///
    /// Returns 1 for a single click, 2 for a double click, or 3 for a triple
    /// click. Additional clicks in the same run stay clamped at 3.
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
        self.click_count
    }
}

/// Surface-level action after normalized pointer routing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PointerAction {
    /// Primary press plus held button/modifier bits.
    PrimaryDown { x: i32, y: i32, key_state: u32 },
    /// Native double-click press.
    PrimaryDoubleDown { x: i32, y: i32 },
    /// Primary release.
    PrimaryUp { x: i32, y: i32 },
    /// Middle press.
    MiddleDown { x: i32, y: i32 },
    /// Hover or held-button motion.
    Move { x: i32, y: i32, key_state: u32 },
    /// Surface leave.
    Leave,
    /// Pointer capture cancellation.
    Cancel,
    /// Unsupported phase/button combination.
    Ignored,
}

fn compute_native_key_state(intent: PointerIntent) -> u32 {
    u32::from(intent.is_primary_down)
        | (u32::from(intent.is_secondary_down) << 1)
        | (u32::from(intent.is_shift_down) << 2)
        | (u32::from(intent.is_control_down) << 3)
        | (u32::from(intent.is_middle_down) << 4)
}

/// Hovered source line plus exact display row under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LineHover {
    /// Source line resolved through the last painted display row index.
    pub(crate) source_line: u32,
    /// Absolute display row under the cursor.
    pub(crate) display_row: u32,
    /// Whether the cursor is inside this pane's gutter strip.
    pub(crate) in_gutter: bool,
}

/// Visible state of a code-surface copy button.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum CodeCopyFeedback {
    /// No recent copy.
    None,
    /// Recent successful copy.
    Copied,
    /// Recent failed copy.
    Failed,
}

/// Kind of code surface targeted by the copy affordance.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum CodeCopyKind {
    /// Fenced code block.
    Fenced,
    /// Inline code run.
    Inline,
}

/// Live copy-button hover for a fenced block or inline code run.
#[derive(Clone, Debug)]
pub(crate) struct CodeCopyHover {
    /// Code-surface kind.
    pub(crate) kind: CodeCopyKind,
    /// Source byte range start for the outer code surface.
    pub(crate) block_start_byte: usize,
    /// Exclusive source byte range end for the outer code surface.
    pub(crate) block_end_byte: usize,
    /// Inner-content byte range start.
    pub(crate) inner_start_byte: usize,
    /// Exclusive inner-content byte range end.
    pub(crate) inner_end_byte: usize,
    /// Button rectangle in client DIPs `(x, y, width, height)`.
    pub(crate) button_rect: (f32, f32, f32, f32),
    /// Whether the cursor currently sits inside `button_rect`.
    pub(crate) button_hovered: bool,
    /// Cached inner content, without fences or backticks.
    pub(crate) inner_text: String,
    /// Current copy feedback state.
    pub(crate) feedback: CodeCopyFeedback,
}

/// Active vertical-scrollbar thumb drag.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ScrollbarDrag {
    /// Pointer offset from the visible thumb top at button-down.
    pub(crate) thumb_grab_offset_dip: f32,
    /// Last pointer y seen during the drag, in client DIPs.
    pub(crate) last_mouse_y_dip: f32,
    /// Count of processed move samples during this drag.
    pub(crate) move_count: u32,
}

/// Active visual-pipe-table column-resize drag.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TableColumnDrag {
    /// Identifies the table by source `block_range.start`.
    pub(crate) block_start: usize,
    /// Column to the left of the dragged boundary.
    pub(crate) col: u32,
    /// Client x in DIPs at button-down.
    pub(crate) start_client_x: f32,
    /// Column width in DIPs at button-down.
    pub(crate) start_width: f32,
    /// Live width in DIPs as the drag tracks the cursor.
    pub(crate) current_width: f32,
}

/// Direction for vertical text-selection autoscroll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutoscrollDirection {
    /// Scroll toward document start.
    Up,
    /// Scroll toward document end.
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

/// Active vertical autoscroll for a text-selection drag.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Autoscroll {
    /// Last cursor x in client DIPs.
    pub(crate) last_cursor_x: i32,
    /// Last cursor y in client DIPs.
    pub(crate) last_cursor_y: i32,
    /// Scroll direction at the last edge-distance sample.
    pub(crate) direction: AutoscrollDirection,
    /// Positive distance past the body edge in DIPs.
    pub(crate) distance_dip: i32,
    /// Wall-clock millis when this autoscroll run started.
    pub(crate) started_ms: u64,
}

#[cfg(test)]
mod tests {
    use continuity_host::{PointerButton, PointerIntent, PointerPhase};

    use super::{PointerAction, PointerState};

    fn intent(phase: PointerPhase, button: PointerButton) -> PointerIntent {
        PointerIntent {
            x_dip: 12.4,
            y_dip: 8.6,
            button,
            phase,
            click_count: 1,
            is_primary_down: button == PointerButton::Primary,
            is_secondary_down: false,
            is_middle_down: false,
            is_shift_down: true,
            is_control_down: true,
            is_alt_down: false,
        }
    }

    #[test]
    fn click_count_increments_within_window_and_clamps() {
        let mut state = PointerState::default();
        assert_eq!(state.register_click(100, 4), 1);
        assert_eq!(state.register_click(200, 4), 2);
        assert_eq!(state.register_click(300, 4), 3);
        assert_eq!(state.register_click(400, 4), 3);
        assert_eq!(state.register_click(1_000, 4), 1);
    }

    #[test]
    fn click_on_different_line_resets_count() {
        let mut state = PointerState::default();
        state.register_click(100, 4);
        assert_eq!(state.register_click(150, 5), 1);
    }

    #[test]
    fn normalized_primary_down_preserves_coordinates_and_modifier_bits() {
        let mut state = PointerState::default();
        assert_eq!(
            state.route_intent(intent(PointerPhase::Down, PointerButton::Primary)),
            PointerAction::PrimaryDown {
                x: 12,
                y: 9,
                key_state: 0x0001 | 0x0004 | 0x0008,
            }
        );
    }

    #[test]
    fn normalized_capture_loss_routes_to_cancel() {
        let mut state = PointerState::default();
        assert_eq!(
            state.route_intent(intent(PointerPhase::Cancel, PointerButton::None)),
            PointerAction::Cancel
        );
    }
}
