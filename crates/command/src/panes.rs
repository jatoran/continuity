//! Phase 13 — pane manipulation commands.
//!
//! Every command routes through [`crate::view_context::ViewContext`] so a
//! `ui::Window` is the single production implementor. Test stubs that
//! `impl ViewContext for X {}` get default `UnsupportedContext` errors.

use std::sync::Arc;

use crate::id::CommandId;
use crate::registry::{Handler, Registry};
use crate::ContextPredicate;

/// CommandId — split focused pane horizontally (side-by-side columns).
pub const PANE_SPLIT_HORIZONTAL: CommandId = CommandId("pane.split_horizontal");
/// CommandId — split focused pane vertically (stacked rows).
pub const PANE_SPLIT_VERTICAL: CommandId = CommandId("pane.split_vertical");
/// CommandId — close focused pane.
pub const PANE_CLOSE: CommandId = CommandId("pane.close");
/// CommandId — focus geometric neighbor.
pub const PANE_FOCUS_LEFT: CommandId = CommandId("pane.focus_left");
/// CommandId — focus geometric neighbor.
pub const PANE_FOCUS_RIGHT: CommandId = CommandId("pane.focus_right");
/// CommandId — focus geometric neighbor.
pub const PANE_FOCUS_UP: CommandId = CommandId("pane.focus_up");
/// CommandId — focus geometric neighbor.
pub const PANE_FOCUS_DOWN: CommandId = CommandId("pane.focus_down");
/// CommandId — toggle maximize within window.
pub const PANE_MAXIMIZE_TOGGLE: CommandId = CommandId("pane.maximize_toggle");
/// CommandId — resize focused pane horizontally (`+` larger, `-` smaller).
pub const PANE_RESIZE_LEFT: CommandId = CommandId("pane.resize_left");
/// CommandId — resize focused pane horizontally.
pub const PANE_RESIZE_RIGHT: CommandId = CommandId("pane.resize_right");
/// CommandId — resize focused pane vertically.
pub const PANE_RESIZE_UP: CommandId = CommandId("pane.resize_up");
/// CommandId — resize focused pane vertically.
pub const PANE_RESIZE_DOWN: CommandId = CommandId("pane.resize_down");
/// CommandId — `Ctrl+Alt+1` collapse to single pane.
pub const LAYOUT_SINGLE: CommandId = CommandId("pane.layout_single");
/// CommandId — `Ctrl+Alt+2` two columns.
pub const LAYOUT_TWO_COLS: CommandId = CommandId("pane.layout_two_cols");
/// CommandId — `Ctrl+Alt+Shift+2` two rows.
pub const LAYOUT_TWO_ROWS: CommandId = CommandId("pane.layout_two_rows");
/// CommandId — `Ctrl+Alt+3` three columns.
pub const LAYOUT_THREE_COLS: CommandId = CommandId("pane.layout_three_cols");
/// CommandId — `Ctrl+Alt+4` four columns.
pub const LAYOUT_FOUR_COLS: CommandId = CommandId("pane.layout_four_cols");
/// CommandId — `Ctrl+Alt+5` 2×2 grid.
pub const LAYOUT_GRID_2X2: CommandId = CommandId("pane.layout_grid_2x2");
/// CommandId — `Ctrl+Alt+8` 2×4 grid.
pub const LAYOUT_GRID_2X4: CommandId = CommandId("pane.layout_grid_2x4");

/// Default keyboard nudge step for `pane.resize_*` commands, in DIPs.
pub(crate) const RESIZE_STEP_DIP: f32 = 24.0;

/// Register every Phase 13 pane-manipulation command on `reg`.
pub fn register_pane_commands(reg: &mut Registry) {
    let when = ContextPredicate::parse("editor.focused");
    let h_split_h: Handler = Arc::new(|_a, ctx| ctx.pane_split_horizontal());
    let h_split_v: Handler = Arc::new(|_a, ctx| ctx.pane_split_vertical());
    let h_close: Handler = Arc::new(|_a, ctx| ctx.pane_close());
    let h_focus_l: Handler = Arc::new(|_a, ctx| ctx.pane_focus_left());
    let h_focus_r: Handler = Arc::new(|_a, ctx| ctx.pane_focus_right());
    let h_focus_u: Handler = Arc::new(|_a, ctx| ctx.pane_focus_up());
    let h_focus_d: Handler = Arc::new(|_a, ctx| ctx.pane_focus_down());
    let h_max: Handler = Arc::new(|_a, ctx| ctx.pane_maximize_toggle());
    let h_resize_l: Handler = Arc::new(|_a, ctx| ctx.pane_resize("horizontal", -RESIZE_STEP_DIP));
    let h_resize_r: Handler = Arc::new(|_a, ctx| ctx.pane_resize("horizontal", RESIZE_STEP_DIP));
    let h_resize_u: Handler = Arc::new(|_a, ctx| ctx.pane_resize("vertical", -RESIZE_STEP_DIP));
    let h_resize_d: Handler = Arc::new(|_a, ctx| ctx.pane_resize("vertical", RESIZE_STEP_DIP));
    let h_l1: Handler = Arc::new(|_a, ctx| ctx.apply_layout_shortcut(1));
    let h_l2: Handler = Arc::new(|_a, ctx| ctx.apply_layout_shortcut(2));
    let h_l2r: Handler = Arc::new(|_a, ctx| ctx.apply_layout_two_rows());
    let h_l3: Handler = Arc::new(|_a, ctx| ctx.apply_layout_shortcut(3));
    let h_l4: Handler = Arc::new(|_a, ctx| ctx.apply_layout_shortcut(4));
    let h_l5: Handler = Arc::new(|_a, ctx| ctx.apply_layout_shortcut(5));
    let h_l8: Handler = Arc::new(|_a, ctx| ctx.apply_layout_shortcut(8));

    reg.register(PANE_SPLIT_HORIZONTAL, when.clone(), h_split_h);
    reg.register(PANE_SPLIT_VERTICAL, when.clone(), h_split_v);
    reg.register(PANE_CLOSE, when.clone(), h_close);
    reg.register(PANE_FOCUS_LEFT, when.clone(), h_focus_l);
    reg.register(PANE_FOCUS_RIGHT, when.clone(), h_focus_r);
    reg.register(PANE_FOCUS_UP, when.clone(), h_focus_u);
    reg.register(PANE_FOCUS_DOWN, when.clone(), h_focus_d);
    reg.register(PANE_MAXIMIZE_TOGGLE, when.clone(), h_max);
    reg.register(PANE_RESIZE_LEFT, when.clone(), h_resize_l);
    reg.register(PANE_RESIZE_RIGHT, when.clone(), h_resize_r);
    reg.register(PANE_RESIZE_UP, when.clone(), h_resize_u);
    reg.register(PANE_RESIZE_DOWN, when.clone(), h_resize_d);
    reg.register(LAYOUT_SINGLE, when.clone(), h_l1);
    reg.register(LAYOUT_TWO_COLS, when.clone(), h_l2);
    reg.register(LAYOUT_TWO_ROWS, when.clone(), h_l2r);
    reg.register(LAYOUT_THREE_COLS, when.clone(), h_l3);
    reg.register(LAYOUT_FOUR_COLS, when.clone(), h_l4);
    reg.register(LAYOUT_GRID_2X2, when.clone(), h_l5);
    reg.register(LAYOUT_GRID_2X4, when, h_l8);
}
