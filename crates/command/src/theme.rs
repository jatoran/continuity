//! δ.5 theme-management command surface.
//!
//! Today, customizing a theme requires the user to hand-copy a bundled
//! `.toml` from the source tree into `%APPDATA%\continuity\themes\`,
//! rename it, hand-edit hex values, save. The TOML + hot-reload
//! plumbing already works (`SettingsWatcher` → `ActiveTheme::reload`).
//! This module supplies the workflow commands that turn the manual
//! checklist into a guided in-editor flow.
//!
//! Every command dispatches through one of the new
//! [`crate::ViewContext`] methods (`theme_clone_active`, `theme_edit_active`,
//! `theme_duplicate`, `theme_rename`, `theme_delete`, `theme_reveal_folder`,
//! `theme_create_blank`). The single production implementor is
//! `ui::Window`, which holds the file-system and overlay state needed
//! to actually do the work.
//!
//! Commands take their inputs as JSON args so dispatch from the
//! command palette, from a keymap chord with preset args, and from the
//! theme-picker overlay's inline actions all funnel through the same
//! registered handlers.

use std::sync::Arc;

use crate::{CommandId, ContextPredicate, Registry};

macro_rules! theme_id {
    ($name:ident, $id:literal) => {
        #[doc = concat!("Theme-management command id `", $id, "`.")]
        pub const $name: CommandId = CommandId($id);
    };
}

theme_id!(THEME_CLONE, "theme.clone");
theme_id!(THEME_EDIT, "theme.edit");
theme_id!(THEME_DUPLICATE, "theme.duplicate");
theme_id!(THEME_RENAME, "theme.rename");
theme_id!(THEME_DELETE, "theme.delete");
theme_id!(THEME_REVEAL_FOLDER, "theme.reveal_folder");
theme_id!(THEME_CREATE_BLANK, "theme.create_blank");

/// JSON arg helper: pull `name` (or fall back to `new`) out of the args
/// for commands that take a single string. Returns the trimmed value or
/// `None` when the key is missing / empty.
fn parse_name_arg(args: &serde_json::Value) -> Option<String> {
    let key = ["name", "new", "to"]
        .iter()
        .find_map(|k| args.get(*k).and_then(serde_json::Value::as_str))?;
    let trimmed = key.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// JSON arg helper: extract the source theme name for commands that operate
/// on a specific row (`theme.duplicate`, `theme.rename`, `theme.delete`).
/// `None` means "use the currently active theme" (the contract for the
/// chord/palette path).
fn parse_source_arg(args: &serde_json::Value) -> Option<String> {
    let key = ["source", "old", "target"]
        .iter()
        .find_map(|k| args.get(*k).and_then(serde_json::Value::as_str))?;
    let trimmed = key.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Register every δ.5 theme-management command and attach palette
/// descriptions. Each handler delegates to a [`crate::ViewContext`]
/// method whose production implementor is `ui::Window`.
pub fn register_theme_commands(registry: &mut Registry) {
    let focused = ContextPredicate::parse("editor.focused");

    registry.register_palette_safe(
        THEME_CLONE,
        focused.clone(),
        Arc::new(|args, ctx| ctx.theme_clone_active(parse_name_arg(args).as_deref())),
    );
    registry.set_description(
        THEME_CLONE,
        "Theme: clone active theme to a new custom theme",
    );

    registry.register_palette_safe(
        THEME_EDIT,
        focused.clone(),
        Arc::new(|args, ctx| {
            // `theme.edit` with `name = "foo"` opens that specific theme;
            // without args it edits the active theme — the row-action and
            // command-palette paths respectively.
            ctx.theme_edit(parse_name_arg(args).as_deref())
        }),
    );
    registry.set_description(
        THEME_EDIT,
        "Theme: open the active theme's TOML for editing",
    );

    registry.register_palette_safe(
        THEME_DUPLICATE,
        focused.clone(),
        Arc::new(|args, ctx| {
            ctx.theme_duplicate(
                parse_source_arg(args).as_deref(),
                parse_name_arg(args).as_deref(),
            )
        }),
    );
    registry.set_description(
        THEME_DUPLICATE,
        "Theme: duplicate any theme into a new custom theme",
    );

    registry.register_palette_safe(
        THEME_RENAME,
        focused.clone(),
        Arc::new(|args, ctx| {
            ctx.theme_rename(
                parse_source_arg(args).as_deref(),
                parse_name_arg(args).as_deref(),
            )
        }),
    );
    registry.set_description(THEME_RENAME, "Theme: rename a custom theme");

    registry.register_palette_safe(
        THEME_DELETE,
        focused.clone(),
        Arc::new(|args, ctx| ctx.theme_delete(parse_source_arg(args).as_deref())),
    );
    registry.set_description(THEME_DELETE, "Theme: soft-delete a custom theme");

    registry.register_palette_safe(
        THEME_REVEAL_FOLDER,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.theme_reveal_folder()),
    );
    registry.set_description(
        THEME_REVEAL_FOLDER,
        "Theme: reveal the themes folder in Explorer",
    );

    registry.register_palette_safe(
        THEME_CREATE_BLANK,
        focused,
        Arc::new(|args, ctx| ctx.theme_create_blank(parse_name_arg(args).as_deref())),
    );
    registry.set_description(
        THEME_CREATE_BLANK,
        "Theme: create a blank custom theme from the neutral palette",
    );
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use serde_json::{json, Value};

    use super::*;
    use crate::{Context, Error, ViewContext};

    #[derive(Default)]
    struct CapturingCtx {
        clone_calls: RefCell<Vec<Option<String>>>,
        edit_calls: RefCell<Vec<Option<String>>>,
        duplicate_calls: RefCell<Vec<(Option<String>, Option<String>)>>,
        rename_calls: RefCell<Vec<(Option<String>, Option<String>)>>,
        delete_calls: RefCell<Vec<Option<String>>>,
        reveal_calls: RefCell<u32>,
        create_blank_calls: RefCell<Vec<Option<String>>>,
    }

    impl Context for CapturingCtx {
        fn lookup(&self, key: &str) -> Option<&str> {
            (key == "editor.focused").then_some("true")
        }
    }
    impl crate::FindContext for CapturingCtx {}
    impl crate::EditConfigContext for CapturingCtx {}
    impl ViewContext for CapturingCtx {
        fn theme_clone_active(&mut self, name: Option<&str>) -> Result<(), Error> {
            self.clone_calls.borrow_mut().push(name.map(str::to_string));
            Ok(())
        }
        fn theme_edit(&mut self, name: Option<&str>) -> Result<(), Error> {
            self.edit_calls.borrow_mut().push(name.map(str::to_string));
            Ok(())
        }
        fn theme_duplicate(
            &mut self,
            source: Option<&str>,
            new_name: Option<&str>,
        ) -> Result<(), Error> {
            self.duplicate_calls
                .borrow_mut()
                .push((source.map(str::to_string), new_name.map(str::to_string)));
            Ok(())
        }
        fn theme_rename(&mut self, old: Option<&str>, new_name: Option<&str>) -> Result<(), Error> {
            self.rename_calls
                .borrow_mut()
                .push((old.map(str::to_string), new_name.map(str::to_string)));
            Ok(())
        }
        fn theme_delete(&mut self, name: Option<&str>) -> Result<(), Error> {
            self.delete_calls
                .borrow_mut()
                .push(name.map(str::to_string));
            Ok(())
        }
        fn theme_reveal_folder(&mut self) -> Result<(), Error> {
            *self.reveal_calls.borrow_mut() += 1;
            Ok(())
        }
        fn theme_create_blank(&mut self, name: Option<&str>) -> Result<(), Error> {
            self.create_blank_calls
                .borrow_mut()
                .push(name.map(str::to_string));
            Ok(())
        }
    }

    fn registry() -> Registry {
        let mut r = Registry::new();
        register_theme_commands(&mut r);
        r
    }

    #[test]
    fn every_command_registers_palette_safe_with_description() {
        let r = registry();
        for id in [
            THEME_CLONE,
            THEME_EDIT,
            THEME_DUPLICATE,
            THEME_RENAME,
            THEME_DELETE,
            THEME_REVEAL_FOLDER,
            THEME_CREATE_BLANK,
        ] {
            assert!(
                r.is_palette_safe(id.as_str()),
                "{} must be palette-safe",
                id.as_str(),
            );
            assert!(
                r.description(id.as_str()).is_some(),
                "{} must carry a palette description",
                id.as_str(),
            );
        }
    }

    #[test]
    fn clone_forwards_name_arg() {
        let mut ctx = CapturingCtx::default();
        let r = registry();
        r.dispatch(THEME_CLONE, &json!({"name": "my-theme"}), &mut ctx)
            .unwrap();
        r.dispatch(THEME_CLONE, &Value::Null, &mut ctx).unwrap();
        let calls = ctx.clone_calls.borrow();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].as_deref(), Some("my-theme"));
        assert!(calls[1].is_none());
    }

    #[test]
    fn duplicate_forwards_source_and_name() {
        let mut ctx = CapturingCtx::default();
        let r = registry();
        r.dispatch(
            THEME_DUPLICATE,
            &json!({"source": "paper", "name": "paper-mine"}),
            &mut ctx,
        )
        .unwrap();
        let calls = ctx.duplicate_calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0.as_deref(), Some("paper"));
        assert_eq!(calls[0].1.as_deref(), Some("paper-mine"));
    }

    #[test]
    fn rename_forwards_old_and_new_under_alt_keys() {
        let mut ctx = CapturingCtx::default();
        let r = registry();
        r.dispatch(THEME_RENAME, &json!({"old": "foo", "new": "bar"}), &mut ctx)
            .unwrap();
        let calls = ctx.rename_calls.borrow();
        assert_eq!(calls[0].0.as_deref(), Some("foo"));
        assert_eq!(calls[0].1.as_deref(), Some("bar"));
    }

    #[test]
    fn delete_forwards_target() {
        let mut ctx = CapturingCtx::default();
        let r = registry();
        r.dispatch(THEME_DELETE, &json!({"target": "mytheme"}), &mut ctx)
            .unwrap();
        let calls = ctx.delete_calls.borrow();
        assert_eq!(calls[0].as_deref(), Some("mytheme"));
    }

    #[test]
    fn reveal_folder_dispatches() {
        let mut ctx = CapturingCtx::default();
        let r = registry();
        r.dispatch(THEME_REVEAL_FOLDER, &Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(*ctx.reveal_calls.borrow(), 1);
    }

    #[test]
    fn create_blank_forwards_name() {
        let mut ctx = CapturingCtx::default();
        let r = registry();
        r.dispatch(THEME_CREATE_BLANK, &json!({"name": "fresh"}), &mut ctx)
            .unwrap();
        let calls = ctx.create_blank_calls.borrow();
        assert_eq!(calls[0].as_deref(), Some("fresh"));
    }

    #[test]
    fn whitespace_only_arg_treated_as_missing() {
        let mut ctx = CapturingCtx::default();
        let r = registry();
        r.dispatch(THEME_CLONE, &json!({"name": "   "}), &mut ctx)
            .unwrap();
        assert!(ctx.clone_calls.borrow()[0].is_none());
    }
}
