//! Phase 12 `settings.*` commands.
//!
//! Today the surface is small: `settings.open` shells out to the OS to
//! open `settings.toml` in whichever editor the user's `.toml` file
//! association points at. The matching native dialog is intentionally
//! deferred — TOML editing is the source of truth and the OS file
//! association already gives users a UX that respects their toolchain.

use std::sync::Arc;

use crate::{CommandId, ContextPredicate, Registry};

/// Command id for `settings.open`.
pub const SETTINGS_OPEN: CommandId = CommandId("settings.open");

/// Command id for `keymap.reload_layered` — invoked by the file watcher
/// when `keymap.toml` changes. Distinct from the Phase-4 `keymap.reload`
/// (which only reloads the default-toml-only baseline) so test stubs can
/// implement one without the other.
pub const KEYMAP_RELOAD_LAYERED: CommandId = CommandId("keymap.reload_layered");

/// Register the `settings.*` command surface.
pub fn register_settings_commands(registry: &mut Registry) {
    let always = ContextPredicate::parse("true");
    let focused = ContextPredicate::parse("editor.focused");

    registry.register(
        SETTINGS_OPEN,
        always,
        Arc::new(|_args, ctx| ctx.open_settings()),
    );
    registry.register(
        KEYMAP_RELOAD_LAYERED,
        focused,
        Arc::new(|_args, ctx| crate::Context::reload_keymap(ctx)),
    );
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use serde_json::Value;

    use super::*;
    use crate::{Context, Error};

    struct StubCtx {
        open_calls: Cell<u32>,
        reload_calls: Cell<u32>,
    }

    impl crate::view_context::ViewContext for StubCtx {
        fn open_settings(&mut self) -> Result<(), Error> {
            self.open_calls.set(self.open_calls.get() + 1);
            Ok(())
        }
    }

    impl crate::FindContext for StubCtx {}
    impl crate::EditConfigContext for StubCtx {}

    impl Context for StubCtx {
        fn lookup(&self, key: &str) -> Option<&str> {
            if key == "editor.focused" {
                Some("true")
            } else {
                None
            }
        }
        fn reload_keymap(&mut self) -> Result<(), Error> {
            self.reload_calls.set(self.reload_calls.get() + 1);
            Ok(())
        }
    }

    #[test]
    fn settings_open_dispatches() {
        let mut registry = Registry::new();
        register_settings_commands(&mut registry);
        let mut ctx = StubCtx {
            open_calls: Cell::new(0),
            reload_calls: Cell::new(0),
        };
        registry
            .dispatch(SETTINGS_OPEN, &Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(ctx.open_calls.get(), 1);
    }

    #[test]
    fn keymap_reload_dispatches() {
        let mut registry = Registry::new();
        register_settings_commands(&mut registry);
        let mut ctx = StubCtx {
            open_calls: Cell::new(0),
            reload_calls: Cell::new(0),
        };
        registry
            .dispatch(KEYMAP_RELOAD_LAYERED, &Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(ctx.reload_calls.get(), 1);
    }
}
