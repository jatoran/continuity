//! Diagnostics commands.

use std::sync::Arc;

use crate::{CommandId, ContextPredicate, Registry};

/// Capture a layout/system diagnostic snapshot to disk.
pub const DIAGNOSTICS_CAPTURE_LAYOUT: CommandId = CommandId("diagnostics.capture_layout");

/// Register diagnostics command handlers.
pub fn register_diagnostics_commands(registry: &mut Registry) {
    let focused = ContextPredicate::parse("editor.focused");
    registry.register(
        DIAGNOSTICS_CAPTURE_LAYOUT,
        focused,
        Arc::new(|_args, ctx| ctx.capture_layout_diagnostics()),
    );
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use serde_json::Value;

    use super::*;
    use crate::{Context, Error};

    struct StubCtx {
        capture_calls: Cell<u32>,
    }

    impl crate::view_context::ViewContext for StubCtx {
        fn capture_layout_diagnostics(&mut self) -> Result<(), Error> {
            self.capture_calls.set(self.capture_calls.get() + 1);
            Ok(())
        }
    }

    impl crate::FindContext for StubCtx {}

    impl Context for StubCtx {
        fn lookup(&self, key: &str) -> Option<&str> {
            if key == "editor.focused" {
                Some("true")
            } else {
                None
            }
        }
    }

    #[test]
    fn diagnostics_capture_layout_dispatches() {
        let mut registry = Registry::new();
        register_diagnostics_commands(&mut registry);
        let mut ctx = StubCtx {
            capture_calls: Cell::new(0),
        };
        registry
            .dispatch(DIAGNOSTICS_CAPTURE_LAYOUT, &Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(ctx.capture_calls.get(), 1);
    }
}
