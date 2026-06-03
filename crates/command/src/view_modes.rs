//! View-mode commands: focus modes (spec §H1), distraction-free (§H2),
//! indent folding (§H3), slash commands (§H5).
//!
//! Pulled out of [`crate::view`] so that file stays under the 600-line
//! cap. Outline-manipulation chords (H4) wire to the existing
//! `markdown.*_section` commands in [`crate::markdown`]; Ctrl+Tab
//! positional (H6) wires the existing `tab.next` / `tab.prev` to
//! `ctrl+tab` directly in `default.toml` plus an overlay HUD scaffold.

use std::sync::Arc;

use crate::{CommandId, ContextPredicate, Registry};

// Re-use the view_id! macro shape inline; the public ids live here so
// they can be re-exported from `crate` next to the other view ids.
macro_rules! view_id {
    ($name:ident, $id:literal) => {
        #[doc = concat!("Phase H view command id `", $id, "`.")]
        pub const $name: CommandId = CommandId($id);
    };
}

// §H1: granular focus mode.
view_id!(VIEW_FOCUS_OFF, "view.focus_off");
view_id!(VIEW_FOCUS_LINE, "view.focus_line");
view_id!(VIEW_FOCUS_SENTENCE, "view.focus_sentence");
view_id!(VIEW_FOCUS_PARAGRAPH, "view.focus_paragraph");
view_id!(VIEW_CYCLE_FOCUS, "view.cycle_focus");
// §H2: distraction-free mode. `view.toggle_distraction_free` is the
// proper home for the chrome-suppressing F11 toggle. The legacy
// `view.toggle_focus_mode` id is retained as a synonym for
// `view.cycle_focus` so existing user keymaps don't break — the audit
// caught that the old wiring routed `toggle_focus_mode` to the
// distraction-free handler, which is a different feature.
view_id!(VIEW_TOGGLE_DISTRACTION_FREE, "view.toggle_distraction_free");
view_id!(VIEW_TOGGLE_FOCUS_MODE, "view.toggle_focus_mode");
// §H3: indent folding.
view_id!(VIEW_FOLD, "view.fold");
view_id!(VIEW_UNFOLD, "view.unfold");
view_id!(VIEW_FOLD_ALL, "view.fold_all");
view_id!(VIEW_UNFOLD_ALL, "view.unfold_all");
view_id!(VIEW_TOGGLE_FOLD_AT_CARET, "view.toggle_fold_at_caret");
// §H5: slash-command palette.
view_id!(VIEW_SLASH_PALETTE_SHOW, "view.slash_palette_show");
// §H6: Ctrl+Tab positional overlay.
view_id!(VIEW_TAB_OVERLAY_SHOW, "view.tab_overlay_show");
// δ.4: previous-buffer browser overlay.
view_id!(
    VIEW_PREVIOUS_BUFFER_BROWSER_SHOW,
    "view.previous_buffer_browser_show"
);

/// Register every Phase-H view command. Called from
/// [`crate::view::register_view_commands`].
pub fn register_view_modes_commands(registry: &mut Registry, focused: &ContextPredicate) {
    // §H1 — focus modes.
    registry.register(
        VIEW_FOCUS_OFF,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.set_focus_mode("off")),
    );
    registry.register(
        VIEW_FOCUS_LINE,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.set_focus_mode("line")),
    );
    registry.register(
        VIEW_FOCUS_SENTENCE,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.set_focus_mode("sentence")),
    );
    registry.register(
        VIEW_FOCUS_PARAGRAPH,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.set_focus_mode("paragraph")),
    );
    registry.register(
        VIEW_CYCLE_FOCUS,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.cycle_focus_mode()),
    );

    // §H2 — distraction-free. The dedicated id routes to the
    // distraction-free handler; `view.toggle_focus_mode` is retained
    // as a synonym for `view.cycle_focus` so callers expecting "step
    // the focus mode forward" still work.
    registry.register(
        VIEW_TOGGLE_DISTRACTION_FREE,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.toggle_distraction_free_mode()),
    );
    registry.register(
        VIEW_TOGGLE_FOCUS_MODE,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.cycle_focus_mode()),
    );

    // §H3 — indent folding.
    registry.register(
        VIEW_FOLD,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.fold_at_caret()),
    );
    registry.register(
        VIEW_UNFOLD,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.unfold_at_caret()),
    );
    registry.register(
        VIEW_FOLD_ALL,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.fold_all()),
    );
    registry.register(
        VIEW_UNFOLD_ALL,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.unfold_all()),
    );
    registry.register(
        VIEW_TOGGLE_FOLD_AT_CARET,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.toggle_fold_at_caret()),
    );

    // §H5 — slash-command palette explicit-show entry point. The
    // typed `/` slash trigger lives in the editor input path; this
    // command lets users open the palette from any chord too.
    registry.register(
        VIEW_SLASH_PALETTE_SHOW,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.show_slash_palette()),
    );

    // §H6 — Ctrl+Tab transient overlay. Released on Ctrl-up; the
    // dispatch path lives in the UI thread's key-event handler. This
    // command is the discoverable palette equivalent.
    registry.register(
        VIEW_TAB_OVERLAY_SHOW,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.show_tab_overlay()),
    );

    // δ.4 — previous-buffer browser overlay. Lists every buffer in the
    // persist DB so the user can reopen a closed buffer beyond the
    // single-step `tab.reopen_closed` history.
    registry.register(
        VIEW_PREVIOUS_BUFFER_BROWSER_SHOW,
        focused.clone(),
        Arc::new(|_args, ctx| ctx.show_previous_buffer_browser()),
    );
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::{Context, Error};

    #[derive(Default)]
    struct StubCtx {
        focus_calls: Cell<u32>,
        last_focus_mode: std::cell::RefCell<String>,
        df_toggle_calls: Cell<u32>,
        fold_calls: Cell<u32>,
        slash_calls: Cell<u32>,
        tab_overlay_calls: Cell<u32>,
    }
    impl Context for StubCtx {
        fn lookup(&self, key: &str) -> Option<&str> {
            (key == "editor.focused").then_some("true")
        }
    }
    impl crate::FindContext for StubCtx {}
    impl crate::EditConfigContext for StubCtx {}
    impl crate::ViewContext for StubCtx {
        fn set_focus_mode(&mut self, mode: &str) -> Result<(), Error> {
            self.focus_calls.set(self.focus_calls.get() + 1);
            *self.last_focus_mode.borrow_mut() = mode.to_string();
            Ok(())
        }
        fn cycle_focus_mode(&mut self) -> Result<(), Error> {
            self.focus_calls.set(self.focus_calls.get() + 1);
            Ok(())
        }
        fn toggle_distraction_free_mode(&mut self) -> Result<(), Error> {
            self.df_toggle_calls.set(self.df_toggle_calls.get() + 1);
            Ok(())
        }
        fn fold_at_caret(&mut self) -> Result<(), Error> {
            self.fold_calls.set(self.fold_calls.get() + 1);
            Ok(())
        }
        fn unfold_at_caret(&mut self) -> Result<(), Error> {
            self.fold_calls.set(self.fold_calls.get() + 1);
            Ok(())
        }
        fn fold_all(&mut self) -> Result<(), Error> {
            self.fold_calls.set(self.fold_calls.get() + 1);
            Ok(())
        }
        fn unfold_all(&mut self) -> Result<(), Error> {
            self.fold_calls.set(self.fold_calls.get() + 1);
            Ok(())
        }
        fn toggle_fold_at_caret(&mut self) -> Result<(), Error> {
            self.fold_calls.set(self.fold_calls.get() + 1);
            Ok(())
        }
        fn show_slash_palette(&mut self) -> Result<(), Error> {
            self.slash_calls.set(self.slash_calls.get() + 1);
            Ok(())
        }
        fn show_tab_overlay(&mut self) -> Result<(), Error> {
            self.tab_overlay_calls.set(self.tab_overlay_calls.get() + 1);
            Ok(())
        }
    }

    fn make_registry() -> Registry {
        let mut reg = Registry::new();
        let focused = ContextPredicate::parse("editor.focused");
        register_view_modes_commands(&mut reg, &focused);
        reg
    }

    #[test]
    fn focus_setters_route_correct_mode_string() {
        let reg = make_registry();
        let mut ctx = StubCtx::default();
        reg.dispatch(VIEW_FOCUS_OFF, &serde_json::Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(*ctx.last_focus_mode.borrow(), "off");
        reg.dispatch(VIEW_FOCUS_LINE, &serde_json::Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(*ctx.last_focus_mode.borrow(), "line");
        reg.dispatch(VIEW_FOCUS_SENTENCE, &serde_json::Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(*ctx.last_focus_mode.borrow(), "sentence");
        reg.dispatch(VIEW_FOCUS_PARAGRAPH, &serde_json::Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(*ctx.last_focus_mode.borrow(), "paragraph");
    }

    #[test]
    fn cycle_focus_command_is_registered() {
        let reg = make_registry();
        let mut ctx = StubCtx::default();
        reg.dispatch(VIEW_CYCLE_FOCUS, &serde_json::Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(ctx.focus_calls.get(), 1);
    }

    #[test]
    fn toggle_distraction_free_routes_to_distraction_free_handler() {
        let reg = make_registry();
        let mut ctx = StubCtx::default();
        reg.dispatch(
            VIEW_TOGGLE_DISTRACTION_FREE,
            &serde_json::Value::Null,
            &mut ctx,
        )
        .unwrap();
        assert_eq!(ctx.df_toggle_calls.get(), 1);
        assert_eq!(ctx.focus_calls.get(), 0);
    }

    #[test]
    fn toggle_focus_mode_routes_to_focus_cycle_handler() {
        // δ.6 audit fix: the legacy `view.toggle_focus_mode` previously
        // toggled distraction-free, which is a different feature. It now
        // routes to the focus-mode cycle handler.
        let reg = make_registry();
        let mut ctx = StubCtx::default();
        reg.dispatch(VIEW_TOGGLE_FOCUS_MODE, &serde_json::Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(ctx.focus_calls.get(), 1);
        assert_eq!(ctx.df_toggle_calls.get(), 0);
    }

    #[test]
    fn every_fold_command_is_registered() {
        let reg = make_registry();
        let mut ctx = StubCtx::default();
        for id in [
            VIEW_FOLD,
            VIEW_UNFOLD,
            VIEW_FOLD_ALL,
            VIEW_UNFOLD_ALL,
            VIEW_TOGGLE_FOLD_AT_CARET,
        ] {
            reg.dispatch(id, &serde_json::Value::Null, &mut ctx)
                .unwrap();
        }
        assert_eq!(ctx.fold_calls.get(), 5);
    }

    #[test]
    fn slash_palette_command_is_registered() {
        let reg = make_registry();
        let mut ctx = StubCtx::default();
        reg.dispatch(VIEW_SLASH_PALETTE_SHOW, &serde_json::Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(ctx.slash_calls.get(), 1);
    }

    #[test]
    fn tab_switcher_overlay_command_is_registered() {
        let reg = make_registry();
        let mut ctx = StubCtx::default();
        reg.dispatch(VIEW_TAB_OVERLAY_SHOW, &serde_json::Value::Null, &mut ctx)
            .unwrap();
        assert_eq!(ctx.tab_overlay_calls.get(), 1);
    }
}
