//! `view.buffer_history` command — opens the buffer-history swimlane
//! tab.
//!
//! Sibling of [`crate::help`] (same shape: one command id +
//! [`crate::Registry::register_palette_safe`] registration) — kept in
//! its own file so the command-id namespace stays one-concept-per-file
//! per crate conventions.

use std::sync::Arc;

use crate::{CommandId, ContextPredicate, Registry};

/// Command id for opening / focusing the buffer-history tab.
///
/// Routed through
/// [`BufferHistoryContext::show_buffer_history_tab`] (default chord
/// `Ctrl+Shift+H`).
pub const VIEW_BUFFER_HISTORY: CommandId = CommandId("view.buffer_history");

/// Register every buffer-history-tab command.
///
/// `view.buffer_history` is `palette_safe = true` — the command only
/// opens a non-buffer visualization tab and never mutates a buffer.
pub fn register_buffer_history_commands(registry: &mut Registry) {
    let focused = ContextPredicate::parse("editor.focused");
    registry.register_palette_safe(
        VIEW_BUFFER_HISTORY,
        focused,
        Arc::new(|_args, ctx| ctx.show_buffer_history_tab()),
    );
    registry.set_description(
        VIEW_BUFFER_HISTORY,
        "Open the buffer-history tab — a horizontal swimlane chart with one row per persisted buffer and snapshot dots along a shared time axis",
    );
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::{Context, Error, ViewContext};

    #[derive(Default)]
    struct StubCtx {
        calls: Cell<u32>,
    }
    impl Context for StubCtx {
        fn lookup(&self, key: &str) -> Option<&str> {
            (key == "editor.focused").then_some("true")
        }
    }
    impl crate::FindContext for StubCtx {}
    impl ViewContext for StubCtx {
        fn show_buffer_history_tab(&mut self) -> Result<(), Error> {
            self.calls.set(self.calls.get() + 1);
            Ok(())
        }
    }

    fn make_registry() -> Registry {
        let mut reg = Registry::new();
        register_buffer_history_commands(&mut reg);
        reg
    }

    #[test]
    fn view_buffer_history_command_is_registered() {
        let reg = make_registry();
        let mut ctx = StubCtx::default();
        reg.dispatch(VIEW_BUFFER_HISTORY, &serde_json::Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(ctx.calls.get(), 1);
    }

    #[test]
    fn view_buffer_history_has_palette_description() {
        let reg = make_registry();
        let description = reg.description(VIEW_BUFFER_HISTORY.0).unwrap();
        assert!(description.contains("buffer"));
        assert!(description.contains("swimlane"));
    }

    #[test]
    fn view_buffer_history_is_palette_safe() {
        let reg = make_registry();
        assert!(reg.is_palette_safe(VIEW_BUFFER_HISTORY.0));
    }
}
