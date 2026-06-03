//! Phase 8 commands: open the various search / palette / goto overlays,
//! plus find-bar navigation and replace.

use std::sync::Arc;

use crate::{CommandId, ContextPredicate, Registry};

/// Open the in-buffer find bar.
pub const EDITOR_FIND: CommandId = CommandId("editor.find");
/// Open the in-buffer find bar with the replace field visible.
pub const EDITOR_REPLACE: CommandId = CommandId("editor.replace");
/// Step to the next find-bar match.
pub const EDITOR_FIND_NEXT: CommandId = CommandId("editor.find_next");
/// Step to the previous find-bar match.
pub const EDITOR_FIND_PREV: CommandId = CommandId("editor.find_prev");
/// Replace the currently-highlighted match with the find bar's replace text.
pub const EDITOR_FIND_REPLACE_ONE: CommandId = CommandId("editor.find_replace_one");
/// Replace every find match (one undo group per buffer).
pub const EDITOR_FIND_REPLACE_ALL: CommandId = CommandId("editor.find_replace_all");
/// Open the find-in-all-buffers panel.
pub const EDITOR_FIND_IN_ALL: CommandId = CommandId("editor.find_in_all");
/// Open the quick-open buffer switcher.
pub const QUICK_OPEN_SHOW: CommandId = CommandId("quick_open.show");
/// Open the command palette.
pub const PALETTE_SHOW: CommandId = CommandId("palette.show");
/// Open the goto-line dialog.
pub const EDITOR_GOTO_LINE: CommandId = CommandId("editor.goto_line");
/// Open the goto-heading dialog.
pub const EDITOR_GOTO_HEADING: CommandId = CommandId("editor.goto_heading");
/// Dismiss any active overlay.
pub const OVERLAY_DISMISS: CommandId = CommandId("overlay.dismiss");
/// G1: flip find-bar case-sensitive mode.
pub(crate) const EDITOR_FIND_TOGGLE_CASE: CommandId = CommandId("editor.find_toggle_case");
/// G1: flip find-bar whole-word mode.
pub(crate) const EDITOR_FIND_TOGGLE_WORD: CommandId = CommandId("editor.find_toggle_word");
/// G1: flip find-bar regex mode.
pub(crate) const EDITOR_FIND_TOGGLE_REGEX: CommandId = CommandId("editor.find_toggle_regex");
/// G1: flip find-bar preserve-case replace mode.
pub(crate) const EDITOR_FIND_TOGGLE_PRESERVE_CASE: CommandId =
    CommandId("editor.find_toggle_preserve_case");
/// G2: flip find-bar buffer/selection scope.
pub(crate) const EDITOR_FIND_TOGGLE_SCOPE: CommandId = CommandId("editor.find_toggle_scope");
/// G3: convert every find-bar match into a cursor and close the bar.
pub(crate) const EDITOR_FIND_MATCHES_TO_CURSORS: CommandId =
    CommandId("editor.find_matches_to_cursors");
/// G3: skip the current match — drop the primary cursor and advance
/// to the next occurrence (Sublime-style).
pub(crate) const EDITOR_SKIP_CURRENT_MATCH: CommandId = CommandId("editor.skip_current_match");

/// Register the Phase 8 search/palette/goto command handlers on `registry`.
pub fn register_search_commands(registry: &mut Registry) {
    let always = ContextPredicate::always();
    let focused = ContextPredicate::parse("editor.focused");

    registry.register(
        EDITOR_FIND,
        focused.clone(),
        Arc::new(|_, ctx| ctx.open_find(false)),
    );
    registry.register(
        EDITOR_REPLACE,
        focused.clone(),
        Arc::new(|_, ctx| ctx.open_find(true)),
    );
    registry.register(
        EDITOR_FIND_NEXT,
        always.clone(),
        Arc::new(|_, ctx| ctx.find_step(1)),
    );
    registry.register(
        EDITOR_FIND_PREV,
        always.clone(),
        Arc::new(|_, ctx| ctx.find_step(-1)),
    );
    registry.register(
        EDITOR_FIND_REPLACE_ONE,
        always.clone(),
        Arc::new(|_, ctx| ctx.find_replace_one()),
    );
    registry.register(
        EDITOR_FIND_REPLACE_ALL,
        always.clone(),
        Arc::new(|_, ctx| ctx.find_replace_all()),
    );
    registry.register(
        EDITOR_FIND_IN_ALL,
        focused.clone(),
        Arc::new(|_, ctx| ctx.open_find_in_all()),
    );
    registry.register(
        QUICK_OPEN_SHOW,
        always.clone(),
        Arc::new(|_, ctx| ctx.open_quick_open()),
    );
    registry.register(
        PALETTE_SHOW,
        always.clone(),
        Arc::new(|_, ctx| ctx.open_palette()),
    );
    registry.register(
        EDITOR_GOTO_LINE,
        focused.clone(),
        Arc::new(|_, ctx| ctx.open_goto_line()),
    );
    registry.register(
        EDITOR_GOTO_HEADING,
        focused,
        Arc::new(|_, ctx| ctx.open_goto_heading()),
    );
    registry.register(
        OVERLAY_DISMISS,
        always.clone(),
        Arc::new(|_, ctx| ctx.dismiss_overlay()),
    );

    // G1 — gated on `find_bar.visible` so Alt+C/W/R chords are inert
    // when the bar isn't open. All three dispatch through a single
    // `Context::find_toggle(mode)` so the trait surface stays tight.
    let fv = ContextPredicate::parse("find_bar.visible");
    registry.register(
        EDITOR_FIND_TOGGLE_CASE,
        fv.clone(),
        Arc::new(|_, ctx| ctx.find_toggle("case")),
    );
    registry.register(
        EDITOR_FIND_TOGGLE_WORD,
        fv.clone(),
        Arc::new(|_, ctx| ctx.find_toggle("word")),
    );
    registry.register(
        EDITOR_FIND_TOGGLE_REGEX,
        fv.clone(),
        Arc::new(|_, ctx| ctx.find_toggle("regex")),
    );
    registry.register(
        EDITOR_FIND_TOGGLE_PRESERVE_CASE,
        fv.clone(),
        Arc::new(|_, ctx| ctx.find_toggle("preserve")),
    );
    registry.register(
        EDITOR_FIND_TOGGLE_SCOPE,
        fv.clone(),
        Arc::new(|_, ctx| ctx.find_toggle("scope")),
    );

    // G3 — Alt+Enter (find-bar-gated) and Ctrl+K Ctrl+D (always).
    registry.register(
        EDITOR_FIND_MATCHES_TO_CURSORS,
        fv,
        Arc::new(|_, ctx| ctx.find_matches_to_cursors()),
    );
    registry.register(
        EDITOR_SKIP_CURRENT_MATCH,
        ContextPredicate::always(),
        Arc::new(|_, ctx| ctx.skip_current_match()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Context;
    use serde_json::Value;

    #[derive(Default)]
    struct StubContext {
        opened_find: bool,
        opened_palette: bool,
        opened_quick: bool,
        opened_goto_line: bool,
        opened_goto_heading: bool,
        opened_find_in_all: bool,
        replace_visible: bool,
        steps: Vec<i32>,
        replaced_one: bool,
        replaced_all: bool,
        dismissed: bool,
        find_bar_visible: bool,
        toggled: Vec<String>,
        matches_to_cursors: u32,
        skipped: u32,
    }

    impl Context for StubContext {
        fn lookup(&self, key: &str) -> Option<&str> {
            match key {
                "editor.focused" => Some("true"),
                "find_bar.visible" if self.find_bar_visible => Some("true"),
                _ => None,
            }
        }
        fn open_find(&mut self, with_replace: bool) -> Result<(), crate::Error> {
            self.opened_find = true;
            self.replace_visible = with_replace;
            Ok(())
        }
        fn open_palette(&mut self) -> Result<(), crate::Error> {
            self.opened_palette = true;
            Ok(())
        }
        fn open_quick_open(&mut self) -> Result<(), crate::Error> {
            self.opened_quick = true;
            Ok(())
        }
        fn open_goto_line(&mut self) -> Result<(), crate::Error> {
            self.opened_goto_line = true;
            Ok(())
        }
        fn open_goto_heading(&mut self) -> Result<(), crate::Error> {
            self.opened_goto_heading = true;
            Ok(())
        }
        fn open_find_in_all(&mut self) -> Result<(), crate::Error> {
            self.opened_find_in_all = true;
            Ok(())
        }
        fn dismiss_overlay(&mut self) -> Result<(), crate::Error> {
            self.dismissed = true;
            Ok(())
        }
    }
    impl crate::ViewContext for StubContext {}
    impl crate::EditConfigContext for StubContext {}
    impl crate::FindContext for StubContext {
        fn find_step(&mut self, delta: i32) -> Result<(), crate::Error> {
            self.steps.push(delta);
            Ok(())
        }
        fn find_replace_one(&mut self) -> Result<(), crate::Error> {
            self.replaced_one = true;
            Ok(())
        }
        fn find_replace_all(&mut self) -> Result<(), crate::Error> {
            self.replaced_all = true;
            Ok(())
        }
        fn find_toggle(&mut self, mode: &str) -> Result<(), crate::Error> {
            self.toggled.push(mode.to_string());
            Ok(())
        }
        fn find_matches_to_cursors(&mut self) -> Result<(), crate::Error> {
            self.matches_to_cursors += 1;
            Ok(())
        }
        fn skip_current_match(&mut self) -> Result<(), crate::Error> {
            self.skipped += 1;
            Ok(())
        }
    }

    #[test]
    fn dispatches_open_find() {
        let mut r = Registry::new();
        register_search_commands(&mut r);
        let mut ctx = StubContext::default();
        let h = r.handler_for_name(EDITOR_FIND.as_str(), &ctx).unwrap();
        h(&Value::Null, &mut ctx).unwrap();
        assert!(ctx.opened_find);
        assert!(!ctx.replace_visible);
    }

    #[test]
    fn dispatches_open_replace_with_flag() {
        let mut r = Registry::new();
        register_search_commands(&mut r);
        let mut ctx = StubContext::default();
        let h = r.handler_for_name(EDITOR_REPLACE.as_str(), &ctx).unwrap();
        h(&Value::Null, &mut ctx).unwrap();
        assert!(ctx.replace_visible);
    }

    #[test]
    fn dispatches_navigation_commands() {
        let mut r = Registry::new();
        register_search_commands(&mut r);
        let mut ctx = StubContext::default();
        let next = r.handler_for_name(EDITOR_FIND_NEXT.as_str(), &ctx).unwrap();
        let prev = r.handler_for_name(EDITOR_FIND_PREV.as_str(), &ctx).unwrap();
        next(&Value::Null, &mut ctx).unwrap();
        prev(&Value::Null, &mut ctx).unwrap();
        assert_eq!(ctx.steps, vec![1, -1]);
    }

    #[test]
    fn dispatches_open_palette_and_quick_open() {
        let mut r = Registry::new();
        register_search_commands(&mut r);
        let mut ctx = StubContext::default();
        let palette = r.handler_for_name(PALETTE_SHOW.as_str(), &ctx).unwrap();
        let quick = r.handler_for_name(QUICK_OPEN_SHOW.as_str(), &ctx).unwrap();
        palette(&Value::Null, &mut ctx).unwrap();
        quick(&Value::Null, &mut ctx).unwrap();
        assert!(ctx.opened_palette);
        assert!(ctx.opened_quick);
    }

    #[test]
    fn find_toggles_gated_by_find_bar_visible() {
        let mut r = Registry::new();
        register_search_commands(&mut r);
        let mut ctx = StubContext::default();
        // Bar hidden → predicate fails.
        assert!(r
            .handler_for_name(EDITOR_FIND_TOGGLE_CASE.as_str(), &ctx)
            .is_err());

        // Bar visible → modes dispatch through find_toggle().
        ctx.find_bar_visible = true;
        for cmd in [
            EDITOR_FIND_TOGGLE_CASE,
            EDITOR_FIND_TOGGLE_WORD,
            EDITOR_FIND_TOGGLE_REGEX,
            EDITOR_FIND_TOGGLE_PRESERVE_CASE,
            EDITOR_FIND_TOGGLE_SCOPE,
        ] {
            let h = r.handler_for_name(cmd.as_str(), &ctx).unwrap();
            h(&Value::Null, &mut ctx).unwrap();
        }
        assert_eq!(
            ctx.toggled,
            vec!["case", "word", "regex", "preserve", "scope"]
        );
    }

    #[test]
    fn find_matches_to_cursors_gated_skip_always() {
        let mut r = Registry::new();
        register_search_commands(&mut r);
        let mut ctx = StubContext::default();
        // matches_to_cursors needs find_bar.visible.
        assert!(r
            .handler_for_name(EDITOR_FIND_MATCHES_TO_CURSORS.as_str(), &ctx)
            .is_err());
        ctx.find_bar_visible = true;
        let h = r
            .handler_for_name(EDITOR_FIND_MATCHES_TO_CURSORS.as_str(), &ctx)
            .unwrap();
        h(&Value::Null, &mut ctx).unwrap();
        assert_eq!(ctx.matches_to_cursors, 1);

        // skip_current_match dispatches regardless of find-bar state.
        ctx.find_bar_visible = false;
        let h = r
            .handler_for_name(EDITOR_SKIP_CURRENT_MATCH.as_str(), &ctx)
            .unwrap();
        h(&Value::Null, &mut ctx).unwrap();
        assert_eq!(ctx.skipped, 1);
    }

    #[test]
    fn dispatches_goto_and_dismiss() {
        let mut r = Registry::new();
        register_search_commands(&mut r);
        let mut ctx = StubContext::default();
        for name in [
            EDITOR_GOTO_LINE.as_str(),
            EDITOR_GOTO_HEADING.as_str(),
            OVERLAY_DISMISS.as_str(),
        ] {
            let h = r.handler_for_name(name, &ctx).unwrap();
            h(&Value::Null, &mut ctx).unwrap();
        }
        assert!(ctx.opened_goto_line);
        assert!(ctx.opened_goto_heading);
        assert!(ctx.dismissed);
    }
}
