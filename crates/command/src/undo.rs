//! Phase 7 — undo / redo / redo_alternate_branch / undo_tree_pick command
//! registrations. The handlers are thin shims over the [`Context`] trait;
//! the real work lives on `core::UndoOrchestrator`.

use std::sync::Arc;

use crate::{CommandId, ContextPredicate, Registry};

/// Undo the most-recent edit group on the active buffer.
pub const EDITOR_UNDO: CommandId = CommandId("editor.undo");
/// Redo the most-recent child of the active buffer's undo head.
pub const EDITOR_REDO: CommandId = CommandId("editor.redo");
/// Redo through an alternate sibling branch of the current group.
pub const EDITOR_REDO_ALTERNATE_BRANCH: CommandId = CommandId("editor.redo_alternate_branch");
/// Open the undo-tree picker (logs the head + immediate children for now;
/// the palette UI ships in Phase 8).
pub const EDITOR_UNDO_TREE_PICK: CommandId = CommandId("editor.undo_tree_pick");

/// Register the four Phase 7 undo commands.
pub fn register_undo_commands(registry: &mut Registry) {
    let focused = ContextPredicate::parse("editor.focused");
    registry.register(EDITOR_UNDO, focused.clone(), Arc::new(|_, ctx| ctx.undo()));
    registry.register(EDITOR_REDO, focused.clone(), Arc::new(|_, ctx| ctx.redo()));
    registry.register(
        EDITOR_REDO_ALTERNATE_BRANCH,
        focused.clone(),
        Arc::new(|_, ctx| ctx.redo_alternate_branch()),
    );
    registry.register(
        EDITOR_UNDO_TREE_PICK,
        focused,
        Arc::new(|_, ctx| ctx.undo_tree_pick()),
    );
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::{Context, Error};

    #[derive(Default)]
    struct StubCtx {
        undos: u32,
        redos: u32,
        alternates: u32,
        picks: u32,
    }

    impl Context for StubCtx {
        fn lookup(&self, key: &str) -> Option<&str> {
            match key {
                "editor.focused" => Some("true"),
                _ => None,
            }
        }

        fn undo(&mut self) -> Result<(), Error> {
            self.undos += 1;
            Ok(())
        }

        fn redo(&mut self) -> Result<(), Error> {
            self.redos += 1;
            Ok(())
        }

        fn redo_alternate_branch(&mut self) -> Result<(), Error> {
            self.alternates += 1;
            Ok(())
        }

        fn undo_tree_pick(&mut self) -> Result<(), Error> {
            self.picks += 1;
            Ok(())
        }
    }
    impl crate::ViewContext for StubCtx {}
    impl crate::FindContext for StubCtx {}
    impl crate::EditConfigContext for StubCtx {}

    #[test]
    fn registered_handlers_dispatch_into_context() {
        let mut registry = Registry::new();
        register_undo_commands(&mut registry);
        let mut ctx = StubCtx::default();
        registry
            .dispatch(EDITOR_UNDO, &Value::Null, &mut ctx)
            .expect("undo");
        registry
            .dispatch(EDITOR_REDO, &Value::Null, &mut ctx)
            .expect("redo");
        registry
            .dispatch(EDITOR_REDO_ALTERNATE_BRANCH, &Value::Null, &mut ctx)
            .expect("redo alt");
        registry
            .dispatch(EDITOR_UNDO_TREE_PICK, &Value::Null, &mut ctx)
            .expect("pick");
        assert_eq!(ctx.undos, 1);
        assert_eq!(ctx.redos, 1);
        assert_eq!(ctx.alternates, 1);
        assert_eq!(ctx.picks, 1);
    }
}
