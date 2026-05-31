//! Phase 13 — tab manipulation commands.
//!
//! Tab navigation (next/prev/positional/MRU), close, reopen, and new-buffer
//! commands. Each delegates to a `ViewContext` method.

use std::sync::Arc;

use crate::id::CommandId;
use crate::registry::Registry;
use crate::ContextPredicate;

/// CommandId — open a fresh empty buffer as a new tab.
pub const TAB_NEW: CommandId = CommandId("tab.new");
/// CommandId — close active tab in focused pane.
pub const TAB_CLOSE: CommandId = CommandId("tab.close");
/// CommandId — step to next positional tab (wraps).
pub const TAB_NEXT: CommandId = CommandId("tab.next");
/// CommandId — step to previous positional tab (wraps).
pub const TAB_PREV: CommandId = CommandId("tab.prev");
/// CommandId — Ctrl+Tab MRU step forward.
pub const TAB_MRU_NEXT: CommandId = CommandId("tab.mru_next");
/// CommandId — Ctrl+Shift+Tab MRU step backward.
pub const TAB_MRU_PREV: CommandId = CommandId("tab.mru_prev");
/// CommandId — reopen the most-recently-closed tab.
pub const TAB_REOPEN_CLOSED: CommandId = CommandId("tab.reopen_closed");
/// CommandId — Ctrl+1 .. Ctrl+9 jump to positional tab.
pub const TAB_GO_TO_1: CommandId = CommandId("tab.go_to_1");
/// CommandId — Ctrl+2 jump to positional tab.
pub const TAB_GO_TO_2: CommandId = CommandId("tab.go_to_2");
/// CommandId — Ctrl+3 jump to positional tab.
pub const TAB_GO_TO_3: CommandId = CommandId("tab.go_to_3");
/// CommandId — Ctrl+4 jump to positional tab.
pub const TAB_GO_TO_4: CommandId = CommandId("tab.go_to_4");
/// CommandId — Ctrl+5 jump to positional tab.
pub const TAB_GO_TO_5: CommandId = CommandId("tab.go_to_5");
/// CommandId — Ctrl+6 jump to positional tab.
pub const TAB_GO_TO_6: CommandId = CommandId("tab.go_to_6");
/// CommandId — Ctrl+7 jump to positional tab.
pub const TAB_GO_TO_7: CommandId = CommandId("tab.go_to_7");
/// CommandId — Ctrl+8 jump to positional tab.
pub const TAB_GO_TO_8: CommandId = CommandId("tab.go_to_8");
/// CommandId — Ctrl+9 jump to positional tab.
pub const TAB_GO_TO_9: CommandId = CommandId("tab.go_to_9");
/// δ.1 — toggle pinned state on the active tab. Pinned tabs anchor
/// leftmost, prefix with a pin glyph, and survive close-others passes.
pub const TAB_PIN_TOGGLE: CommandId = CommandId("tab.pin_toggle");

/// Register every Phase 13 tab-manipulation command on `reg`.
pub fn register_tab_commands(reg: &mut Registry) {
    let when = ContextPredicate::parse("editor.focused");
    reg.register(TAB_NEW, when.clone(), Arc::new(|_a, c| c.tab_new()));
    reg.register(TAB_CLOSE, when.clone(), Arc::new(|_a, c| c.tab_close()));
    reg.register(TAB_NEXT, when.clone(), Arc::new(|_a, c| c.tab_next()));
    reg.register(TAB_PREV, when.clone(), Arc::new(|_a, c| c.tab_prev()));
    reg.register(
        TAB_MRU_NEXT,
        when.clone(),
        Arc::new(|_a, c| c.tab_step_mru(1)),
    );
    reg.register(
        TAB_MRU_PREV,
        when.clone(),
        Arc::new(|_a, c| c.tab_step_mru(-1)),
    );
    reg.register(
        TAB_REOPEN_CLOSED,
        when.clone(),
        Arc::new(|_a, c| c.tab_reopen_closed()),
    );
    macro_rules! go_to {
        ($id:expr, $n:expr) => {
            reg.register($id, when.clone(), Arc::new(|_a, c| c.tab_go_to($n)));
        };
    }
    go_to!(TAB_GO_TO_1, 1);
    go_to!(TAB_GO_TO_2, 2);
    go_to!(TAB_GO_TO_3, 3);
    go_to!(TAB_GO_TO_4, 4);
    go_to!(TAB_GO_TO_5, 5);
    go_to!(TAB_GO_TO_6, 6);
    go_to!(TAB_GO_TO_7, 7);
    go_to!(TAB_GO_TO_8, 8);
    go_to!(TAB_GO_TO_9, 9);
    reg.register(TAB_PIN_TOGGLE, when, Arc::new(|_a, c| c.tab_pin_toggle()));
}
