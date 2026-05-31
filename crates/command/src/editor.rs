//! Phase 4 baseline editor commands and keymap maintenance.
//!
//! Phase 6 motion extras and rich editing live in [`crate::editor_extras`].

use std::sync::Arc;

use continuity_core::SelectionEdit;
use serde_json::Value;

use crate::{CommandId, ContextPredicate, Error, Registry};

/// Insert a character at the active caret.
pub const EDITOR_INSERT_CHAR: CommandId = CommandId("editor.insert_char");
/// Insert a newline at the active caret.
pub const EDITOR_INSERT_NEWLINE: CommandId = CommandId("editor.insert_newline");
/// Delete one character before the active caret.
pub const EDITOR_DELETE_BACK: CommandId = CommandId("editor.delete_back");
/// Delete one character after the active caret.
pub const EDITOR_DELETE_FORWARD: CommandId = CommandId("editor.delete_forward");
/// Move the active caret one character forward.
pub const EDITOR_MOVE_CHAR_FORWARD: CommandId = CommandId("editor.move_char_forward");
/// Move the active caret one character backward.
pub const EDITOR_MOVE_CHAR_BACKWARD: CommandId = CommandId("editor.move_char_backward");
/// Move the active caret one line up.
pub const EDITOR_MOVE_LINE_UP: CommandId = CommandId("editor.move_line_up");
/// Move the active caret one line down.
pub const EDITOR_MOVE_LINE_DOWN: CommandId = CommandId("editor.move_line_down");
/// Move the active caret to the start of its line.
pub const EDITOR_MOVE_LINE_START: CommandId = CommandId("editor.move_line_start");
/// Move the active caret to the end of its line.
pub const EDITOR_MOVE_LINE_END: CommandId = CommandId("editor.move_line_end");
/// Move the active caret to the start of the document.
pub const EDITOR_MOVE_DOC_START: CommandId = CommandId("editor.move_doc_start");
/// Move the active caret to the end of the document.
pub const EDITOR_MOVE_DOC_END: CommandId = CommandId("editor.move_doc_end");
/// Log all keymap conflicts for the active keymap.
pub const KEYMAP_SHOW_CONFLICTS: CommandId = CommandId("keymap.show_conflicts");
/// Reload the keymap from its configured source.
pub const KEYMAP_RELOAD: CommandId = CommandId("keymap.reload");
/// δ.1 — jump the primary caret back to the most-recently-edited
/// position in this buffer. Per-buffer cursor stack; repeated
/// invocations walk further back through the history.
pub const EDITOR_GOTO_LAST_EDIT: CommandId = CommandId("editor.goto_last_edit");

/// Register Phase 4 baseline editor commands.
pub fn register_editor_primitives(registry: &mut Registry) {
    let focused = ContextPredicate::parse("editor.focused");
    // Phase-16.5: insert_char first checks the active context's
    // auto-pair configuration. When the typed char is a configured
    // open delimiter, dispatch a pair-insert that lands as one undo
    // group with the caret between the new pair. Otherwise fall
    // through to a plain insert.
    registry.register(
        EDITOR_INSERT_CHAR,
        focused.clone(),
        Arc::new(|args, ctx| {
            let s = insert_char_arg(args)?;
            let c = s.chars().next().expect("insert_char_arg validated len 1");
            if let Some((open, close)) = ctx.auto_pair_for(c) {
                return ctx.apply_selection_edit(SelectionEdit::InsertPair {
                    open: open.to_string(),
                    close: close.to_string(),
                });
            }
            ctx.insert_text(s)
        }),
    );
    registry.register(
        EDITOR_INSERT_NEWLINE,
        focused.clone(),
        Arc::new(|_, ctx| ctx.insert_text("\n")),
    );
    // Phase-16.5: delete_back first asks the context whether the
    // caret sits between a configured empty pair. When `true` the
    // pair-delete already ran (one undo group); otherwise fall
    // through to the legacy single-char delete.
    registry.register(
        EDITOR_DELETE_BACK,
        focused.clone(),
        Arc::new(|_, ctx| {
            if ctx.try_delete_back_pair()? {
                return Ok(());
            }
            ctx.delete_back()
        }),
    );
    registry.register(
        EDITOR_DELETE_FORWARD,
        focused.clone(),
        Arc::new(|_, ctx| ctx.delete_forward()),
    );
    registry.register(
        EDITOR_MOVE_CHAR_FORWARD,
        focused.clone(),
        Arc::new(|_, ctx| ctx.move_char(1)),
    );
    registry.register(
        EDITOR_MOVE_CHAR_BACKWARD,
        focused.clone(),
        Arc::new(|_, ctx| ctx.move_char(-1)),
    );
    registry.register(
        EDITOR_MOVE_LINE_UP,
        focused.clone(),
        Arc::new(|_, ctx| ctx.move_line(-1)),
    );
    registry.register(
        EDITOR_MOVE_LINE_DOWN,
        focused.clone(),
        Arc::new(|_, ctx| ctx.move_line(1)),
    );
    registry.register(
        EDITOR_MOVE_LINE_START,
        focused.clone(),
        Arc::new(|_, ctx| ctx.move_line_start()),
    );
    registry.register(
        EDITOR_MOVE_LINE_END,
        focused.clone(),
        Arc::new(|_, ctx| ctx.move_line_end()),
    );
    registry.register(
        EDITOR_MOVE_DOC_START,
        focused.clone(),
        Arc::new(|_, ctx| ctx.move_doc_start()),
    );
    registry.register(
        EDITOR_MOVE_DOC_END,
        focused.clone(),
        Arc::new(|_, ctx| ctx.move_doc_end()),
    );
    registry.register(
        EDITOR_GOTO_LAST_EDIT,
        focused,
        Arc::new(|_, ctx| ctx.goto_last_edit()),
    );
}

/// Register Phase 4 keymap maintenance commands.
pub fn register_keymap_commands(registry: &mut Registry) {
    let focused = ContextPredicate::parse("editor.focused");
    registry.register(
        KEYMAP_SHOW_CONFLICTS,
        focused.clone(),
        Arc::new(|_, ctx| ctx.show_keymap_conflicts()),
    );
    registry.register(
        KEYMAP_RELOAD,
        focused,
        Arc::new(|_, ctx| ctx.reload_keymap()),
    );
}

fn insert_char_arg(args: &Value) -> Result<&str, Error> {
    let Some(s) = args.as_str() else {
        return Err(Error::InvalidArgs {
            name: EDITOR_INSERT_CHAR.as_str(),
            reason: "expected JSON string".into(),
        });
    };
    if s.chars().count() != 1 {
        return Err(Error::InvalidArgs {
            name: EDITOR_INSERT_CHAR.as_str(),
            reason: "expected exactly one character".into(),
        });
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Context;

    pub(super) struct StubCtx {
        pub text: String,
        pub cursor: usize,
        pub focused: bool,
        pub conflicts_shown: bool,
        pub reloaded: bool,
    }

    impl StubCtx {
        pub fn new(text: &str) -> Self {
            Self {
                text: text.into(),
                cursor: 0,
                focused: true,
                conflicts_shown: false,
                reloaded: false,
            }
        }
    }

    impl Context for StubCtx {
        fn lookup(&self, key: &str) -> Option<&str> {
            match (key, self.focused) {
                ("editor.focused", true) => Some("true"),
                ("selection.is_caret", _) => Some("true"),
                ("language", _) => Some("plain"),
                _ => None,
            }
        }

        fn insert_text(&mut self, text: &str) -> Result<(), Error> {
            self.text.insert_str(self.cursor, text);
            self.cursor += text.len();
            Ok(())
        }

        fn delete_back(&mut self) -> Result<(), Error> {
            if self.cursor > 0 {
                self.cursor -= 1;
                self.text.remove(self.cursor);
            }
            Ok(())
        }

        fn delete_forward(&mut self) -> Result<(), Error> {
            if self.cursor < self.text.len() {
                self.text.remove(self.cursor);
            }
            Ok(())
        }

        fn move_char(&mut self, delta: i32) -> Result<(), Error> {
            if delta < 0 {
                self.cursor = self.cursor.saturating_sub(delta.unsigned_abs() as usize);
            } else {
                self.cursor = (self.cursor + delta as usize).min(self.text.len());
            }
            Ok(())
        }

        fn move_line(&mut self, delta: i32) -> Result<(), Error> {
            self.move_char(delta)
        }

        fn move_line_start(&mut self) -> Result<(), Error> {
            self.cursor = self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1);
            Ok(())
        }

        fn move_line_end(&mut self) -> Result<(), Error> {
            self.cursor = self.text[self.cursor..]
                .find('\n')
                .map_or(self.text.len(), |i| self.cursor + i);
            Ok(())
        }

        fn move_doc_start(&mut self) -> Result<(), Error> {
            self.cursor = 0;
            Ok(())
        }

        fn move_doc_end(&mut self) -> Result<(), Error> {
            self.cursor = self.text.len();
            Ok(())
        }

        fn show_keymap_conflicts(&mut self) -> Result<(), Error> {
            self.conflicts_shown = true;
            Ok(())
        }

        fn reload_keymap(&mut self) -> Result<(), Error> {
            self.reloaded = true;
            Ok(())
        }
    }
    impl crate::ViewContext for StubCtx {}
    impl crate::FindContext for StubCtx {}

    pub(super) fn registry() -> Registry {
        let mut registry = Registry::new();
        register_editor_primitives(&mut registry);
        register_keymap_commands(&mut registry);
        registry
    }

    #[test]
    fn baseline_editor_commands_mutate_stub_context() {
        let registry = registry();
        let mut ctx = StubCtx::new("");

        registry
            .dispatch(EDITOR_INSERT_CHAR, &Value::String("a".into()), &mut ctx)
            .expect("insert ok");
        registry
            .dispatch(EDITOR_INSERT_NEWLINE, &Value::Null, &mut ctx)
            .expect("newline ok");
        registry
            .dispatch(EDITOR_INSERT_CHAR, &Value::String("b".into()), &mut ctx)
            .expect("insert ok");
        assert_eq!(ctx.text, "a\nb");

        registry
            .dispatch(EDITOR_MOVE_DOC_START, &Value::Null, &mut ctx)
            .expect("doc start");
        registry
            .dispatch(EDITOR_MOVE_CHAR_FORWARD, &Value::Null, &mut ctx)
            .expect("char fwd");
        registry
            .dispatch(EDITOR_DELETE_FORWARD, &Value::Null, &mut ctx)
            .expect("delete forward");
        assert_eq!(ctx.text, "ab");

        registry
            .dispatch(EDITOR_MOVE_DOC_END, &Value::Null, &mut ctx)
            .expect("doc end");
        registry
            .dispatch(EDITOR_MOVE_CHAR_BACKWARD, &Value::Null, &mut ctx)
            .expect("char back");
        registry
            .dispatch(EDITOR_DELETE_BACK, &Value::Null, &mut ctx)
            .expect("delete back");
        assert_eq!(ctx.text, "b");
    }

    #[test]
    fn line_and_keymap_commands_dispatch() {
        let registry = registry();
        let mut ctx = StubCtx::new("ab\ncd");
        ctx.cursor = 4;

        registry
            .dispatch(EDITOR_MOVE_LINE_START, &Value::Null, &mut ctx)
            .expect("line start");
        assert_eq!(ctx.cursor, 3);
        registry
            .dispatch(EDITOR_MOVE_LINE_END, &Value::Null, &mut ctx)
            .expect("line end");
        assert_eq!(ctx.cursor, 5);
        registry
            .dispatch(EDITOR_MOVE_LINE_UP, &Value::Null, &mut ctx)
            .expect("line up");
        registry
            .dispatch(EDITOR_MOVE_LINE_DOWN, &Value::Null, &mut ctx)
            .expect("line down");
        registry
            .dispatch(KEYMAP_SHOW_CONFLICTS, &Value::Null, &mut ctx)
            .expect("show conflicts");
        registry
            .dispatch(KEYMAP_RELOAD, &Value::Null, &mut ctx)
            .expect("reload");
        assert!(ctx.conflicts_shown);
        assert!(ctx.reloaded);
    }

    #[test]
    fn insert_char_rejects_invalid_args() {
        let registry = registry();
        let mut ctx = StubCtx::new("");
        let err = registry
            .dispatch(EDITOR_INSERT_CHAR, &Value::String("ab".into()), &mut ctx)
            .unwrap_err();
        assert!(matches!(err, Error::InvalidArgs { .. }));
    }
}
