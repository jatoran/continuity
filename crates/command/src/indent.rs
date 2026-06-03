//! Indentation command family: runtime control over `[editor]`
//! `indent_type` / `indent_width` / `tab_width`.
//!
//! Split out of [`crate::editor_extras`] (one concept per file) so that
//! file stays under the 600-line cap. Each command dispatches through a
//! [`crate::ViewContext`] mutator whose production implementor is
//! `ui::Window`; the window updates its per-window indent mirror, drives
//! `view_options.indent_size` / the rendered tab stop, persists the new
//! value to `settings.toml`, and invalidates the next paint.
//!
//! The `_increase` / `_decrease` / `_use_*` commands are palette-safe
//! (non-destructive, no argument). `editor.set_indent_width` /
//! `editor.set_tab_width` take a JSON `{ "width": N }` argument and are
//! *not* palette-safe — they mirror `view.set_font_size`, which the
//! palette cannot supply an argument for.

use std::sync::Arc;

use crate::{CommandId, ContextPredicate, Registry};

/// `editor.indent_use_spaces` — switch indentation to spaces.
pub const EDITOR_INDENT_USE_SPACES: CommandId = CommandId("editor.indent_use_spaces");
/// `editor.indent_use_tabs` — switch indentation to tabs.
pub const EDITOR_INDENT_USE_TABS: CommandId = CommandId("editor.indent_use_tabs");
/// `editor.indent_width_increase` — `+1` column (clamped `1..=16`).
pub const EDITOR_INDENT_WIDTH_INCREASE: CommandId = CommandId("editor.indent_width_increase");
/// `editor.indent_width_decrease` — `-1` column (clamped `1..=16`).
pub const EDITOR_INDENT_WIDTH_DECREASE: CommandId = CommandId("editor.indent_width_decrease");
/// `editor.tab_width_increase` — `+1` column (clamped `1..=16`).
pub const EDITOR_TAB_WIDTH_INCREASE: CommandId = CommandId("editor.tab_width_increase");
/// `editor.tab_width_decrease` — `-1` column (clamped `1..=16`).
pub const EDITOR_TAB_WIDTH_DECREASE: CommandId = CommandId("editor.tab_width_decrease");
/// `editor.set_indent_width` — explicit width via JSON `{ "width": N }`.
pub const EDITOR_SET_INDENT_WIDTH: CommandId = CommandId("editor.set_indent_width");
/// `editor.set_tab_width` — explicit width via JSON `{ "width": N }`.
pub const EDITOR_SET_TAB_WIDTH: CommandId = CommandId("editor.set_tab_width");

/// JSON arg helper: `{ "width": 4 }`. Clamps to `1..=16` here so a
/// malformed or out-of-range argument can never push the runtime mirror
/// outside the validated range. Missing / non-integer arguments fall
/// back to `4` (the default), mirroring `view.set_font_size`'s
/// fall-back-to-default behaviour.
fn parse_width(args: &serde_json::Value) -> u32 {
    args.get("width")
        .and_then(serde_json::Value::as_u64)
        .map(|n| n.clamp(1, 16) as u32)
        .unwrap_or(4)
}

/// Register the indentation command family.
pub fn register_indent_commands(registry: &mut Registry) {
    let focused = ContextPredicate::parse("editor.focused");

    registry.register(
        EDITOR_INDENT_USE_SPACES,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.set_indent_type(true)),
    );
    registry.mark_palette_safe(EDITOR_INDENT_USE_SPACES);
    registry.set_description(EDITOR_INDENT_USE_SPACES, "Use spaces for indentation");

    registry.register(
        EDITOR_INDENT_USE_TABS,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.set_indent_type(false)),
    );
    registry.mark_palette_safe(EDITOR_INDENT_USE_TABS);
    registry.set_description(EDITOR_INDENT_USE_TABS, "Use tabs for indentation");

    registry.register(
        EDITOR_INDENT_WIDTH_INCREASE,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.adjust_indent_width(1)),
    );
    registry.mark_palette_safe(EDITOR_INDENT_WIDTH_INCREASE);
    registry.set_description(EDITOR_INDENT_WIDTH_INCREASE, "Increase indent width");

    registry.register(
        EDITOR_INDENT_WIDTH_DECREASE,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.adjust_indent_width(-1)),
    );
    registry.mark_palette_safe(EDITOR_INDENT_WIDTH_DECREASE);
    registry.set_description(EDITOR_INDENT_WIDTH_DECREASE, "Decrease indent width");

    registry.register(
        EDITOR_TAB_WIDTH_INCREASE,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.adjust_tab_width(1)),
    );
    registry.mark_palette_safe(EDITOR_TAB_WIDTH_INCREASE);
    registry.set_description(EDITOR_TAB_WIDTH_INCREASE, "Increase tab width");

    registry.register(
        EDITOR_TAB_WIDTH_DECREASE,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.adjust_tab_width(-1)),
    );
    registry.mark_palette_safe(EDITOR_TAB_WIDTH_DECREASE);
    registry.set_description(EDITOR_TAB_WIDTH_DECREASE, "Decrease tab width");

    // JSON-arg variants. NOT palette-safe — the palette can't supply
    // the `{ width }` argument (same rationale as `view.set_font_size`).
    registry.register(
        EDITOR_SET_INDENT_WIDTH,
        focused.clone(),
        Arc::new(|args, ctx| ctx.set_indent_width(parse_width(args))),
    );
    registry.register(
        EDITOR_SET_TAB_WIDTH,
        focused.clone(),
        Arc::new(|args, ctx| ctx.set_tab_width(parse_width(args))),
    );
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use serde_json::Value;

    use super::*;
    use crate::{Context, Error};

    #[derive(Default)]
    struct IndentCtx {
        last_use_spaces: Cell<Option<bool>>,
        indent_delta_total: Cell<i32>,
        tab_delta_total: Cell<i32>,
        last_set_indent_width: Cell<Option<u32>>,
        last_set_tab_width: Cell<Option<u32>>,
    }
    impl Context for IndentCtx {
        fn lookup(&self, key: &str) -> Option<&str> {
            (key == "editor.focused").then_some("true")
        }
    }
    impl crate::FindContext for IndentCtx {}
    impl crate::EditConfigContext for IndentCtx {}
    impl crate::ViewContext for IndentCtx {
        fn set_indent_type(&mut self, use_spaces: bool) -> Result<(), Error> {
            self.last_use_spaces.set(Some(use_spaces));
            Ok(())
        }
        fn adjust_indent_width(&mut self, delta: i32) -> Result<(), Error> {
            self.indent_delta_total
                .set(self.indent_delta_total.get() + delta);
            Ok(())
        }
        fn adjust_tab_width(&mut self, delta: i32) -> Result<(), Error> {
            self.tab_delta_total.set(self.tab_delta_total.get() + delta);
            Ok(())
        }
        fn set_indent_width(&mut self, width: u32) -> Result<(), Error> {
            self.last_set_indent_width.set(Some(width));
            Ok(())
        }
        fn set_tab_width(&mut self, width: u32) -> Result<(), Error> {
            self.last_set_tab_width.set(Some(width));
            Ok(())
        }
    }

    fn registry() -> Registry {
        let mut registry = Registry::new();
        register_indent_commands(&mut registry);
        registry
    }

    #[test]
    fn use_spaces_and_tabs_dispatch_to_context() {
        let registry = registry();
        let mut ctx = IndentCtx::default();
        registry
            .dispatch(EDITOR_INDENT_USE_SPACES, &Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(ctx.last_use_spaces.get(), Some(true));
        registry
            .dispatch(EDITOR_INDENT_USE_TABS, &Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(ctx.last_use_spaces.get(), Some(false));
    }

    #[test]
    fn width_inc_dec_dispatch_signed_deltas() {
        let registry = registry();
        let mut ctx = IndentCtx::default();
        registry
            .dispatch(EDITOR_INDENT_WIDTH_INCREASE, &Value::Null, &mut ctx)
            .unwrap();
        registry
            .dispatch(EDITOR_INDENT_WIDTH_DECREASE, &Value::Null, &mut ctx)
            .unwrap();
        registry
            .dispatch(EDITOR_INDENT_WIDTH_INCREASE, &Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(ctx.indent_delta_total.get(), 1);
        registry
            .dispatch(EDITOR_TAB_WIDTH_INCREASE, &Value::Null, &mut ctx)
            .unwrap();
        registry
            .dispatch(EDITOR_TAB_WIDTH_DECREASE, &Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(ctx.tab_delta_total.get(), 0);
    }

    #[test]
    fn set_width_parses_and_clamps_argument() {
        let registry = registry();
        let mut ctx = IndentCtx::default();
        registry
            .dispatch(
                EDITOR_SET_INDENT_WIDTH,
                &serde_json::json!({ "width": 8 }),
                &mut ctx,
            )
            .unwrap();
        assert_eq!(ctx.last_set_indent_width.get(), Some(8));
        // Out-of-range clamps to 16.
        registry
            .dispatch(
                EDITOR_SET_TAB_WIDTH,
                &serde_json::json!({ "width": 99 }),
                &mut ctx,
            )
            .unwrap();
        assert_eq!(ctx.last_set_tab_width.get(), Some(16));
        // Missing argument falls back to the default 4.
        registry
            .dispatch(EDITOR_SET_INDENT_WIDTH, &Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(ctx.last_set_indent_width.get(), Some(4));
    }

    #[test]
    fn arg_variants_are_not_palette_safe() {
        let registry = registry();
        assert!(registry.is_palette_safe(EDITOR_INDENT_USE_SPACES.as_str()));
        assert!(registry.is_palette_safe(EDITOR_TAB_WIDTH_INCREASE.as_str()));
        assert!(!registry.is_palette_safe(EDITOR_SET_INDENT_WIDTH.as_str()));
        assert!(!registry.is_palette_safe(EDITOR_SET_TAB_WIDTH.as_str()));
    }

    /// `editor.indent` / `editor.outdent` / `editor.spaces_to_tabs` /
    /// `editor.tabs_to_spaces` (registered in [`crate::editor_extras`])
    /// must build their `SelectionEdit` from the live indent config read
    /// off the dispatch context, not a registration-time constant.
    #[test]
    fn indent_handlers_read_context_indent_config() {
        use continuity_core::{IndentUnit, SelectionEdit};

        #[derive(Default)]
        struct IndentEditCaptor {
            last: Option<SelectionEdit>,
        }
        impl Context for IndentEditCaptor {
            fn lookup(&self, key: &str) -> Option<&str> {
                (key == "editor.focused").then_some("true")
            }
            fn apply_selection_edit(&mut self, edit: SelectionEdit) -> Result<(), Error> {
                self.last = Some(edit);
                Ok(())
            }
        }
        impl crate::ViewContext for IndentEditCaptor {}
        impl crate::FindContext for IndentEditCaptor {}
        impl crate::EditConfigContext for IndentEditCaptor {
            fn indent_unit(&self) -> IndentUnit {
                IndentUnit::Spaces(2)
            }
            fn effective_tab_width(&self) -> u32 {
                8
            }
        }

        let mut registry = Registry::new();
        crate::editor_extras::register_rich_editing(&mut registry);
        let mut ctx = IndentEditCaptor::default();

        registry
            .dispatch(crate::editor_extras::EDITOR_INDENT, &Value::Null, &mut ctx)
            .expect("dispatch ok");
        assert!(matches!(
            ctx.last,
            Some(SelectionEdit::Indent {
                unit: IndentUnit::Spaces(2)
            })
        ));
        registry
            .dispatch(crate::editor_extras::EDITOR_OUTDENT, &Value::Null, &mut ctx)
            .expect("dispatch ok");
        assert!(matches!(
            ctx.last,
            Some(SelectionEdit::Outdent {
                unit: IndentUnit::Spaces(2)
            })
        ));
        registry
            .dispatch(
                crate::editor_extras::EDITOR_SPACES_TO_TABS,
                &Value::Null,
                &mut ctx,
            )
            .expect("dispatch ok");
        assert!(matches!(
            ctx.last,
            Some(SelectionEdit::SpacesToTabs { tab_width: 8 })
        ));
        registry
            .dispatch(
                crate::editor_extras::EDITOR_TABS_TO_SPACES,
                &Value::Null,
                &mut ctx,
            )
            .expect("dispatch ok");
        assert!(matches!(
            ctx.last,
            Some(SelectionEdit::TabsToSpaces { tab_width: 8 })
        ));
    }
}
