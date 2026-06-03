//! Phase 6 cursor-motion + selection extras.
//!
//! Word- and paragraph-scoped motion / extension, plus the
//! "shrink the smart-expanded selection" companion to
//! [`crate::selection::EDITOR_EXPAND_SELECTION_SMART`]. All handlers fan
//! out to per-method `Context::move_*` / `extend_*` calls so the trait
//! surface stays small.
//!
//! Rich-editing commands (newline, delete, surround, sort, etc.) live in
//! [`crate::editor_extras`].

use std::sync::Arc;

use crate::{CommandId, ContextPredicate, Registry};

/// Move the caret one word forward.
pub const EDITOR_MOVE_WORD_FORWARD: CommandId = CommandId("editor.move_word_forward");
/// Move the caret one word backward.
pub const EDITOR_MOVE_WORD_BACKWARD: CommandId = CommandId("editor.move_word_backward");
/// Extend the selection one word forward.
pub const EDITOR_EXTEND_WORD_FORWARD: CommandId = CommandId("editor.extend_word_forward");
/// Extend the selection one word backward.
pub const EDITOR_EXTEND_WORD_BACKWARD: CommandId = CommandId("editor.extend_word_backward");
/// Move the caret one paragraph forward.
pub const EDITOR_MOVE_PARAGRAPH_FORWARD: CommandId = CommandId("editor.move_paragraph_forward");
/// Move the caret one paragraph backward.
pub const EDITOR_MOVE_PARAGRAPH_BACKWARD: CommandId = CommandId("editor.move_paragraph_backward");
/// Extend the selection one paragraph forward.
pub const EDITOR_EXTEND_PARAGRAPH_FORWARD: CommandId = CommandId("editor.extend_paragraph_forward");
/// Extend the selection one paragraph backward.
pub const EDITOR_EXTEND_PARAGRAPH_BACKWARD: CommandId =
    CommandId("editor.extend_paragraph_backward");
/// Shrink the smart-expanded selection by one scope.
pub const EDITOR_SHRINK_SELECTION_SMART: CommandId = CommandId("editor.shrink_selection_smart");
/// Smart-home: toggle caret between column 0 and first-non-whitespace.
pub const EDITOR_SMART_HOME: CommandId = CommandId("editor.smart_home");
/// Smart-home extending the active selection.
pub const EDITOR_EXTEND_SMART_HOME: CommandId = CommandId("editor.extend_smart_home");

/// Register Phase 6 cursor-move + selection extras (word, paragraph,
/// shrink-smart).
pub fn register_motion_extras(registry: &mut Registry) {
    let focused = ContextPredicate::parse("editor.focused");
    let bind = |registry: &mut Registry, id: CommandId, h: crate::registry::Handler| {
        registry.register(id, focused.clone(), h);
    };
    bind(
        registry,
        EDITOR_MOVE_WORD_FORWARD,
        Arc::new(|_, ctx| ctx.move_word(1)),
    );
    bind(
        registry,
        EDITOR_MOVE_WORD_BACKWARD,
        Arc::new(|_, ctx| ctx.move_word(-1)),
    );
    bind(
        registry,
        EDITOR_EXTEND_WORD_FORWARD,
        Arc::new(|_, ctx| ctx.extend_word(1)),
    );
    bind(
        registry,
        EDITOR_EXTEND_WORD_BACKWARD,
        Arc::new(|_, ctx| ctx.extend_word(-1)),
    );
    bind(
        registry,
        EDITOR_MOVE_PARAGRAPH_FORWARD,
        Arc::new(|_, ctx| ctx.move_paragraph(1)),
    );
    bind(
        registry,
        EDITOR_MOVE_PARAGRAPH_BACKWARD,
        Arc::new(|_, ctx| ctx.move_paragraph(-1)),
    );
    bind(
        registry,
        EDITOR_EXTEND_PARAGRAPH_FORWARD,
        Arc::new(|_, ctx| ctx.extend_paragraph(1)),
    );
    bind(
        registry,
        EDITOR_EXTEND_PARAGRAPH_BACKWARD,
        Arc::new(|_, ctx| ctx.extend_paragraph(-1)),
    );
    bind(
        registry,
        EDITOR_SHRINK_SELECTION_SMART,
        Arc::new(|_, ctx| ctx.shrink_selection_smart()),
    );
    bind(
        registry,
        EDITOR_SMART_HOME,
        Arc::new(|_, ctx| ctx.smart_home()),
    );
    bind(
        registry,
        EDITOR_EXTEND_SMART_HOME,
        Arc::new(|_, ctx| ctx.extend_smart_home()),
    );
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::{Context, Error};

    #[derive(Default)]
    struct Captor {
        moves: u32,
    }

    impl Context for Captor {
        fn lookup(&self, key: &str) -> Option<&str> {
            (key == "editor.focused").then_some("true")
        }
        fn move_word(&mut self, _: i32) -> Result<(), Error> {
            self.moves += 1;
            Ok(())
        }
        fn extend_word(&mut self, _: i32) -> Result<(), Error> {
            self.moves += 1;
            Ok(())
        }
        fn move_paragraph(&mut self, _: i32) -> Result<(), Error> {
            self.moves += 1;
            Ok(())
        }
        fn extend_paragraph(&mut self, _: i32) -> Result<(), Error> {
            self.moves += 1;
            Ok(())
        }
        fn shrink_selection_smart(&mut self) -> Result<(), Error> {
            self.moves += 1;
            Ok(())
        }
    }
    impl crate::ViewContext for Captor {}
    impl crate::FindContext for Captor {}
    impl crate::EditConfigContext for Captor {}

    #[test]
    fn motion_extras_dispatch() {
        let mut registry = Registry::new();
        register_motion_extras(&mut registry);
        let mut ctx = Captor::default();
        for id in [
            EDITOR_MOVE_WORD_FORWARD,
            EDITOR_MOVE_WORD_BACKWARD,
            EDITOR_EXTEND_WORD_FORWARD,
            EDITOR_EXTEND_WORD_BACKWARD,
            EDITOR_MOVE_PARAGRAPH_FORWARD,
            EDITOR_MOVE_PARAGRAPH_BACKWARD,
            EDITOR_EXTEND_PARAGRAPH_FORWARD,
            EDITOR_EXTEND_PARAGRAPH_BACKWARD,
            EDITOR_SHRINK_SELECTION_SMART,
        ] {
            registry
                .dispatch(id, &Value::Null, &mut ctx)
                .expect("motion ok");
        }
        assert_eq!(ctx.moves, 9);
    }
}
