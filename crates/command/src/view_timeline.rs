//! Buffer-timeline commands: time-machine slider and named snapshots.
//!
//! Sibling of [`crate::view_modes`]; pulled out of [`crate::view`]
//! to keep that file under the 600-line cap. Each handler delegates
//! through the [`crate::ViewContext`] surface (production
//! implementor: `ui::Window`).

use std::sync::Arc;

use crate::{CommandId, ContextPredicate, Registry};

macro_rules! view_id {
    ($name:ident, $id:literal) => {
        #[doc = concat!("Timeline command id `", $id, "`.")]
        pub const $name: CommandId = CommandId($id);
    };
}

view_id!(BUFFER_TIMELINE, "buffer.timeline");
view_id!(BUFFER_MARK_SNAPSHOT, "buffer.mark_snapshot");

/// Pull a string argument out of a JSON object: `{"label": "draft 1"}`.
fn parse_label(args: &serde_json::Value) -> Option<String> {
    args.get("label")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

/// Register the buffer-timeline commands.
///
/// `buffer.mark_snapshot` is not palette-safe because it expects a
/// contextual label. `buffer.timeline` is a read-only surface.
pub fn register_view_timeline_commands(registry: &mut Registry, focused: &ContextPredicate) {
    registry.register_palette_safe(
        BUFFER_TIMELINE,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.open_buffer_timeline()),
    );
    registry.set_description(
        BUFFER_TIMELINE,
        "Open the time-machine slider for this buffer (drag to preview, Enter restores, Esc cancels)",
    );

    registry.register(
        BUFFER_MARK_SNAPSHOT,
        focused.clone(),
        Arc::new(|args, ctx| {
            let label = parse_label(args).unwrap_or_default();
            ctx.mark_next_snapshot(&label)
        }),
    );
    registry.set_description(
        BUFFER_MARK_SNAPSHOT,
        "Label the next persisted snapshot — takes a string arg `{ \"label\": \"<name>\" }` (e.g. \"draft 1\", \"pre-refactor\"); empty clears any pending label",
    );
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::{Context, Error};

    #[derive(Default)]
    struct StubCtx {
        timeline_calls: Cell<u32>,
        mark_calls: Cell<u32>,
        last_label: std::cell::RefCell<String>,
    }

    impl Context for StubCtx {
        fn lookup(&self, key: &str) -> Option<&str> {
            (key == "editor.focused").then_some("true")
        }
    }

    impl crate::FindContext for StubCtx {}
    impl crate::EditConfigContext for StubCtx {}

    impl crate::ViewContext for StubCtx {
        fn open_buffer_timeline(&mut self) -> Result<(), Error> {
            self.timeline_calls.set(self.timeline_calls.get() + 1);
            Ok(())
        }

        fn mark_next_snapshot(&mut self, label: &str) -> Result<(), Error> {
            self.mark_calls.set(self.mark_calls.get() + 1);
            *self.last_label.borrow_mut() = label.to_owned();
            Ok(())
        }
    }

    fn make_registry() -> Registry {
        let mut registry = Registry::new();
        let focused = ContextPredicate::parse("editor.focused");
        register_view_timeline_commands(&mut registry, &focused);
        registry
    }

    #[test]
    fn buffer_timeline_command_is_registered() {
        let registry = make_registry();
        let mut context = StubCtx::default();
        registry
            .dispatch(BUFFER_TIMELINE, &serde_json::Value::Null, &mut context)
            .unwrap();
        assert_eq!(context.timeline_calls.get(), 1);
    }

    #[test]
    fn buffer_mark_snapshot_passes_label() {
        let registry = make_registry();
        let mut context = StubCtx::default();
        let args = serde_json::json!({ "label": "pre-refactor" });
        registry
            .dispatch(BUFFER_MARK_SNAPSHOT, &args, &mut context)
            .unwrap();
        assert_eq!(context.mark_calls.get(), 1);
        assert_eq!(*context.last_label.borrow(), "pre-refactor");
    }

    #[test]
    fn mark_snapshot_with_no_label_passes_empty_string() {
        let registry = make_registry();
        let mut context = StubCtx::default();
        registry
            .dispatch(BUFFER_MARK_SNAPSHOT, &serde_json::Value::Null, &mut context)
            .unwrap();
        assert_eq!(*context.last_label.borrow(), "");
    }

    #[test]
    fn descriptions_are_registered_for_timeline_commands() {
        let registry = make_registry();
        let mark = registry.description(BUFFER_MARK_SNAPSHOT.0).unwrap();
        assert!(mark.starts_with("Label the next persisted snapshot"));
        assert!(mark.contains("\"label\""));
        let timeline = registry.description(BUFFER_TIMELINE.0).unwrap();
        assert!(timeline.contains("time-machine"));
    }

    #[test]
    fn palette_safe_flags_match_timeline_behavior() {
        let registry = make_registry();
        assert!(registry.is_palette_safe(BUFFER_TIMELINE.0));
        assert!(!registry.is_palette_safe(BUFFER_MARK_SNAPSHOT.0));
    }
}
