//! Phase-16 spell-check commands: per-buffer toggle, replace at caret,
//! and add-to-dictionary.

use std::sync::Arc;

use serde_json::Value;

use crate::{CommandId, Context, ContextPredicate, Error, Registry};

/// Toggle spell-check on the active buffer.
pub const SPELL_TOGGLE: CommandId = CommandId("spell.toggle");
/// Replace the misspelled word under the caret with `args` (string).
pub const SPELL_REPLACE_AT_CARET: CommandId = CommandId("spell.replace_at_caret");
/// Add the word under the caret to the user's spell-check ignore list
/// (in-memory for the session).
pub const SPELL_ADD_TO_DICTIONARY: CommandId = CommandId("spell.add_to_dictionary");
/// Open the suggestion popup at the caret-screen-position for the
/// current misspelled word.
pub const SPELL_SHOW_SUGGESTIONS: CommandId = CommandId("spell.show_suggestions");

/// Register every Phase-16 spell-check command.
pub fn register_spell_commands(reg: &mut Registry) {
    reg.register(
        SPELL_TOGGLE,
        ContextPredicate::parse("editor.focused"),
        Arc::new(|_args: &Value, ctx: &mut dyn Context| ctx.spell_toggle()),
    );
    reg.register(
        SPELL_REPLACE_AT_CARET,
        ContextPredicate::parse("editor.focused"),
        Arc::new(|args: &Value, ctx: &mut dyn Context| {
            let Some(s) = args.as_str() else {
                return Err(Error::InvalidArgs {
                    name: SPELL_REPLACE_AT_CARET.as_str(),
                    reason: "expected replacement string".into(),
                });
            };
            ctx.spell_replace_at_caret(s)
        }),
    );
    reg.register(
        SPELL_ADD_TO_DICTIONARY,
        ContextPredicate::parse("editor.focused"),
        Arc::new(|_args: &Value, ctx: &mut dyn Context| ctx.spell_add_to_dictionary()),
    );
    reg.register(
        SPELL_SHOW_SUGGESTIONS,
        ContextPredicate::parse("editor.focused"),
        Arc::new(|_args: &Value, ctx: &mut dyn Context| ctx.spell_show_suggestions()),
    );
}

/// Diagnostic command-id list.
pub const SPELL_COMMAND_IDS: [&str; 4] = [
    SPELL_TOGGLE.as_str(),
    SPELL_REPLACE_AT_CARET.as_str(),
    SPELL_ADD_TO_DICTIONARY.as_str(),
    SPELL_SHOW_SUGGESTIONS.as_str(),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, ViewContext};

    #[derive(Default)]
    struct Ctx {
        toggled: bool,
        replaced: Option<String>,
        added: bool,
        shown: bool,
    }
    impl ViewContext for Ctx {
        fn spell_toggle(&mut self) -> Result<(), Error> {
            self.toggled = true;
            Ok(())
        }
        fn spell_replace_at_caret(&mut self, with: &str) -> Result<(), Error> {
            self.replaced = Some(with.to_string());
            Ok(())
        }
        fn spell_add_to_dictionary(&mut self) -> Result<(), Error> {
            self.added = true;
            Ok(())
        }
        fn spell_show_suggestions(&mut self) -> Result<(), Error> {
            self.shown = true;
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
        register_spell_commands(&mut reg);
        let mut ctx = Ctx::default();
        reg.dispatch(SPELL_TOGGLE, &Value::Null, &mut ctx).unwrap();
        reg.dispatch(
            SPELL_REPLACE_AT_CARET,
            &Value::String("hello".into()),
            &mut ctx,
        )
        .unwrap();
        reg.dispatch(SPELL_ADD_TO_DICTIONARY, &Value::Null, &mut ctx)
            .unwrap();
        reg.dispatch(SPELL_SHOW_SUGGESTIONS, &Value::Null, &mut ctx)
            .unwrap();
        assert!(ctx.toggled && ctx.added && ctx.shown);
        assert_eq!(ctx.replaced.as_deref(), Some("hello"));
    }
}
