//! Buffer-timeline + view-metrics commands: time-machine slider +
//! named snapshots (spec §I1), WPM + activity heatmap buffer (§I2).
//!
//! Sibling of [`crate::view_modes`]; pulled out of [`crate::view`]
//! to keep that file under the 600-line cap. Each handler delegates
//! through the [`crate::ViewContext`] surface (production
//! implementor: `ui::Window`).

use std::sync::Arc;

use crate::{CommandId, ContextPredicate, Registry};

macro_rules! view_id {
    ($name:ident, $id:literal) => {
        #[doc = concat!("Phase I view command id `", $id, "`.")]
        pub const $name: CommandId = CommandId($id);
    };
}

// §I1 — time-machine.
view_id!(BUFFER_TIMELINE, "buffer.timeline");
view_id!(BUFFER_MARK_SNAPSHOT, "buffer.mark_snapshot");
// §I2 — metrics buffer.
view_id!(VIEW_METRICS, "view.metrics");
view_id!(METRICS_PURGE, "metrics.purge");

/// Pull a string argument out of a JSON object: `{"label": "draft 1"}`.
fn parse_label(args: &serde_json::Value) -> Option<String> {
    args.get("label")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

/// Register every Phase-I view command. Called from
/// [`crate::view::register_view_commands`].
///
/// `buffer.mark_snapshot` and `metrics.purge` are flagged `palette_safe = false`
/// because the former is contextual (asks for a label) and the latter is
/// destructive — see the A7 palette-safe rules. `buffer.timeline` and
/// `view.metrics` are palette-safe (insertion-only / read-only surfaces).
pub fn register_view_timeline_metrics_commands(
    registry: &mut Registry,
    focused: &ContextPredicate,
) {
    // §I1 — open the timeline overlay on the focused pane.
    registry.register_palette_safe(
        BUFFER_TIMELINE,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.open_buffer_timeline()),
    );
    registry.set_description(
        BUFFER_TIMELINE,
        "Open the time-machine slider for this buffer (drag to preview, Enter restores, Esc cancels)",
    );

    // §I1 — label the next snapshot.
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

    // §I2 — open the dedicated metrics buffer as a new tab.
    registry.register_palette_safe(
        VIEW_METRICS,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.show_metrics_buffer()),
    );
    registry.set_description(
        VIEW_METRICS,
        "Open the typing-metrics dashboard (WPM + activity heatmap) as a tab",
    );

    // §I2 — purge every metrics_daily row.
    registry.register(
        METRICS_PURGE,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.purge_metrics()),
    );
    registry.set_description(
        METRICS_PURGE,
        "Drop every recorded daily-metrics row (destructive; not undoable)",
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
        metrics_show_calls: Cell<u32>,
        metrics_purge_calls: Cell<u32>,
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
        fn show_metrics_buffer(&mut self) -> Result<(), Error> {
            self.metrics_show_calls
                .set(self.metrics_show_calls.get() + 1);
            Ok(())
        }
        fn purge_metrics(&mut self) -> Result<(), Error> {
            self.metrics_purge_calls
                .set(self.metrics_purge_calls.get() + 1);
            Ok(())
        }
    }

    fn make_registry() -> Registry {
        let mut reg = Registry::new();
        let focused = ContextPredicate::parse("editor.focused");
        register_view_timeline_metrics_commands(&mut reg, &focused);
        reg
    }

    #[test]
    fn buffer_timeline_command_is_registered() {
        let reg = make_registry();
        let mut ctx = StubCtx::default();
        reg.dispatch(BUFFER_TIMELINE, &serde_json::Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(ctx.timeline_calls.get(), 1);
    }

    #[test]
    fn buffer_mark_snapshot_passes_label() {
        let reg = make_registry();
        let mut ctx = StubCtx::default();
        let args = serde_json::json!({ "label": "pre-refactor" });
        reg.dispatch(BUFFER_MARK_SNAPSHOT, &args, &mut ctx).unwrap();
        assert_eq!(ctx.mark_calls.get(), 1);
        assert_eq!(*ctx.last_label.borrow(), "pre-refactor");
    }

    #[test]
    fn mark_snapshot_with_no_label_passes_empty_string() {
        let reg = make_registry();
        let mut ctx = StubCtx::default();
        reg.dispatch(BUFFER_MARK_SNAPSHOT, &serde_json::Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(*ctx.last_label.borrow(), "");
    }

    #[test]
    fn view_metrics_command_is_registered() {
        let reg = make_registry();
        let mut ctx = StubCtx::default();
        reg.dispatch(VIEW_METRICS, &serde_json::Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(ctx.metrics_show_calls.get(), 1);
    }

    #[test]
    fn metrics_purge_command_is_registered() {
        let reg = make_registry();
        let mut ctx = StubCtx::default();
        reg.dispatch(METRICS_PURGE, &serde_json::Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(ctx.metrics_purge_calls.get(), 1);
    }

    #[test]
    fn descriptions_are_registered_for_every_view_metrics_command() {
        let reg = make_registry();
        // Every Phase-I command should carry a one-line palette
        // description so the command palette / slash palette can
        // surface what it does.
        let mark = reg.description(BUFFER_MARK_SNAPSHOT.0).unwrap();
        assert!(
            mark.starts_with("Label the next persisted snapshot"),
            "buffer.mark_snapshot description should lead with the verb: got {mark:?}",
        );
        assert!(
            mark.contains("\"label\""),
            "buffer.mark_snapshot description should call out the `label` arg: got {mark:?}",
        );
        let timeline = reg.description(BUFFER_TIMELINE.0).unwrap();
        assert!(timeline.contains("time-machine"));
        assert!(reg.description(VIEW_METRICS.0).is_some());
        assert!(reg.description(METRICS_PURGE.0).is_some());
    }

    #[test]
    fn palette_safe_flags_match_doc() {
        let reg = make_registry();
        assert!(reg.is_palette_safe(BUFFER_TIMELINE.0));
        assert!(reg.is_palette_safe(VIEW_METRICS.0));
        // Destructive / contextual stay out of the slash palette.
        assert!(!reg.is_palette_safe(METRICS_PURGE.0));
        assert!(!reg.is_palette_safe(BUFFER_MARK_SNAPSHOT.0));
    }
}
