//! Phase-16 clipboard commands: cut, copy, paste, paste-as-plain-text,
//! paste-from-history.
//!
//! These dispatch through [`crate::ViewContext`] so the production
//! [`crate::Context`] (`ui::Window`) can handle the OS-clipboard
//! interactions; test stubs need no body.

use std::sync::Arc;

use serde_json::Value;

use crate::{CommandId, Context, ContextPredicate, Registry};

/// `Ctrl+X` — copy the primary selection's source text and delete it.
pub const EDITOR_CUT: CommandId = CommandId("editor.cut");
/// `Ctrl+C` — copy the primary selection's source text.
pub const EDITOR_COPY: CommandId = CommandId("editor.copy");
/// `Ctrl+V` — paste clipboard text at the caret(s).
pub const EDITOR_PASTE: CommandId = CommandId("editor.paste");
/// `Ctrl+Shift+V` — paste clipboard text stripped of any rich format
/// (the editor only consumes `CF_UNICODETEXT` so this is a discoverable
/// alias for the same path; future formats can route through here).
pub const EDITOR_PASTE_AS_PLAIN_TEXT: CommandId = CommandId("editor.paste_as_plain_text");
/// `Ctrl+Alt+V` — open the paste-history overlay.
pub const EDITOR_PASTE_FROM_HISTORY: CommandId = CommandId("editor.paste_from_history");
/// δ.1 — `Ctrl+L` quick-yank: copy the caret's line (including its
/// trailing newline) without requiring an explicit selection.
pub const EDITOR_COPY_LINE: CommandId = CommandId("editor.copy_line");

/// Register every Phase-16 clipboard command.
pub fn register_clipboard_commands(reg: &mut Registry) {
    reg.register(
        EDITOR_CUT,
        ContextPredicate::parse("editor.focused"),
        Arc::new(|_args: &Value, ctx: &mut dyn Context| ctx.cut_selection()),
    );
    reg.register(
        EDITOR_COPY,
        ContextPredicate::parse("editor.focused"),
        Arc::new(|_args: &Value, ctx: &mut dyn Context| ctx.copy_selection()),
    );
    reg.register(
        EDITOR_PASTE,
        ContextPredicate::parse("editor.focused"),
        Arc::new(|_args: &Value, ctx: &mut dyn Context| ctx.paste_clipboard()),
    );
    reg.register(
        EDITOR_PASTE_AS_PLAIN_TEXT,
        ContextPredicate::parse("editor.focused"),
        Arc::new(|_args: &Value, ctx: &mut dyn Context| ctx.paste_as_plain_text()),
    );
    reg.register(
        EDITOR_PASTE_FROM_HISTORY,
        ContextPredicate::parse("editor.focused"),
        Arc::new(|args: &Value, ctx: &mut dyn Context| {
            let index = args.as_u64().map(|n| n as usize);
            ctx.paste_from_history(index)
        }),
    );
    reg.register(
        EDITOR_COPY_LINE,
        ContextPredicate::parse("editor.focused"),
        Arc::new(|_args: &Value, ctx: &mut dyn Context| ctx.copy_caret_line()),
    );
}

/// Diagnostic command-id list for tests / palette discovery.
pub const CLIPBOARD_COMMAND_IDS: [&str; 6] = [
    EDITOR_CUT.as_str(),
    EDITOR_COPY.as_str(),
    EDITOR_PASTE.as_str(),
    EDITOR_PASTE_AS_PLAIN_TEXT.as_str(),
    EDITOR_PASTE_FROM_HISTORY.as_str(),
    EDITOR_COPY_LINE.as_str(),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, Error, ViewContext};

    #[derive(Default)]
    struct Ctx {
        cut: bool,
        copied: bool,
        pasted: bool,
        plain: bool,
        history: Option<Option<usize>>,
        copied_line: bool,
    }
    impl ViewContext for Ctx {
        fn cut_selection(&mut self) -> Result<(), Error> {
            self.cut = true;
            Ok(())
        }
        fn copy_selection(&mut self) -> Result<(), Error> {
            self.copied = true;
            Ok(())
        }
        fn paste_clipboard(&mut self) -> Result<(), Error> {
            self.pasted = true;
            Ok(())
        }
        fn paste_as_plain_text(&mut self) -> Result<(), Error> {
            self.plain = true;
            Ok(())
        }
        fn paste_from_history(&mut self, index: Option<usize>) -> Result<(), Error> {
            self.history = Some(index);
            Ok(())
        }
        fn copy_caret_line(&mut self) -> Result<(), Error> {
            self.copied_line = true;
            Ok(())
        }
    }
    impl crate::FindContext for Ctx {}
    impl Context for Ctx {
        fn lookup(&self, key: &str) -> Option<&str> {
            (key == "editor.focused").then_some("true")
        }
    }

    #[test]
    fn dispatches_each_command() {
        let mut reg = Registry::new();
        register_clipboard_commands(&mut reg);
        let mut ctx = Ctx::default();
        reg.dispatch(EDITOR_CUT, &Value::Null, &mut ctx).unwrap();
        reg.dispatch(EDITOR_COPY, &Value::Null, &mut ctx).unwrap();
        reg.dispatch(EDITOR_PASTE, &Value::Null, &mut ctx).unwrap();
        reg.dispatch(EDITOR_PASTE_AS_PLAIN_TEXT, &Value::Null, &mut ctx)
            .unwrap();
        reg.dispatch(
            EDITOR_PASTE_FROM_HISTORY,
            &Value::Number(serde_json::Number::from(2u64)),
            &mut ctx,
        )
        .unwrap();
        reg.dispatch(EDITOR_COPY_LINE, &Value::Null, &mut ctx)
            .unwrap();
        assert!(ctx.cut && ctx.copied && ctx.pasted && ctx.plain && ctx.copied_line);
        assert_eq!(ctx.history, Some(Some(2)));
    }
}
