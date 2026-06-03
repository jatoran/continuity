//! Hold-modifier chord HUD (§E6).
//!
//! When the user holds `Ctrl` / `Alt` / `Shift` for ~600 ms without
//! pressing a non-modifier key, a transient panel appears listing every
//! binding whose first chord matches the held modifier set, scoped to
//! the current `Context`. Releasing the modifier (or pressing any
//! non-modifier key) dismisses the panel — the panel never captures
//! input, so the underlying chord still fires normally.
//!
//! This module ships the **state machine + binding matcher** (§A
//! scaffolding precedent). The WM_TIMER hook that drives the dwell
//! transition and the renderer-side paint glue are documented follow-up
//! sites; see the module-level integration notes at the bottom.
//!
//! Thread ownership: the UI thread of the window the panel is bound to.

use continuity_command::Context;
use continuity_input::Modifiers;
use continuity_keymap::Keymap;

/// Monotonic millisecond timestamp. Callers feed this from whatever
/// clock source they own (the production UI thread uses
/// `crate::wall_clock_ms()`; tests pass synthetic values). Using a
/// plain `u64` here keeps the module independent of `std::time::Instant`
/// (whose constructors are private) and of the `core` crate's `Clock`
/// trait.
pub type TickMs = u64;

/// Dwell time before the HUD appears. Spec §E6 says "~600 ms".
pub const HUD_DELAY_MS: u64 = 600;

/// One row in the HUD list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HudEntry {
    /// Command id (e.g. `"editor.find"`).
    pub command: String,
    /// Rendered chord label (e.g. `"Ctrl+F"`).
    pub chord_label: String,
    /// Command namespace, used to group the two-column layout
    /// (`editor`, `view`, `markdown`, …). Empty when the command id has
    /// no `.` (e.g. `"undo"`).
    pub namespace: String,
}

/// State of the hold-modifier HUD.
#[derive(Clone, Debug, Default)]
pub enum HudState {
    /// No modifier is currently held, or the user has already pressed a
    /// non-modifier key during the current hold.
    #[default]
    Idle,
    /// A modifier is held but the dwell time has not yet elapsed.
    Pending {
        /// Held modifier set at the start of the dwell.
        mods: Modifiers,
        /// Millisecond tick the dwell started.
        since: TickMs,
    },
    /// The dwell has elapsed; the HUD is on screen.
    Active {
        /// Currently held modifier set.
        mods: Modifiers,
        /// Rendered HUD rows in the current `Context`.
        rows: Vec<HudEntry>,
    },
}

/// `true` when any of `ctrl`, `alt`, `shift`, `meta` is held.
#[must_use]
pub fn any_modifier(mods: Modifiers) -> bool {
    mods.ctrl || mods.alt || mods.shift || mods.meta
}

impl HudState {
    /// `true` when the panel is currently visible.
    #[must_use]
    pub fn is_visible(&self) -> bool {
        matches!(self, Self::Active { .. })
    }

    /// Modifier-only key edge: the user pressed or released a modifier
    /// without typing any other key during this hold. `now` is the
    /// current UI tick.
    ///
    /// Behaviour matrix:
    /// - Mods empty → reset to `Idle` (HUD dismisses).
    /// - Mods non-empty and we are `Idle` → enter `Pending` with `since
    ///   = now`.
    /// - Mods non-empty and we are `Pending` with a different mask →
    ///   reset the dwell (chord-HUD only triggers on a *steady* mask).
    /// - Mods non-empty and we are `Active` with the same mask → no-op.
    /// - Mods non-empty and we are `Active` with a different mask →
    ///   drop back to `Pending` so the new mask gets its own dwell.
    pub fn on_modifier_edge(&mut self, mods: Modifiers, now: TickMs) {
        if !any_modifier(mods) {
            *self = Self::Idle;
            return;
        }
        match self {
            Self::Idle => {
                *self = Self::Pending { mods, since: now };
            }
            Self::Pending { mods: cur, since } => {
                if *cur != mods {
                    *self = Self::Pending { mods, since: now };
                } else {
                    // No mask change → preserve `since` so the dwell
                    // timer keeps ticking from the original press.
                    let _ = since;
                }
            }
            Self::Active { mods: cur, .. } => {
                if *cur != mods {
                    *self = Self::Pending { mods, since: now };
                }
            }
        }
    }

    /// A non-modifier key was typed during this hold. The user is
    /// completing a chord, not asking for the cheatsheet — dismiss any
    /// in-flight HUD and suppress this hold from triggering one.
    pub fn on_chord_typed(&mut self) {
        *self = Self::Idle;
    }

    /// Poll the state machine. If the dwell has elapsed while we were
    /// in `Pending`, compute the row list from `keymap` and `ctx`,
    /// transition to `Active`, and return `Some(&rows)`. Returns `None`
    /// when no transition happened — caller need not invalidate the
    /// window.
    pub fn poll(&mut self, now: TickMs, keymap: &Keymap, ctx: &dyn Context) -> Option<&[HudEntry]> {
        let due = matches!(self, Self::Pending { since, .. }
                            if now.saturating_sub(*since) >= HUD_DELAY_MS);
        if !due {
            return None;
        }
        let mods = match self {
            Self::Pending { mods, .. } => *mods,
            _ => return None,
        };
        let rows = matching_bindings(keymap, mods, ctx);
        *self = Self::Active { mods, rows };
        if let Self::Active { rows, .. } = self {
            Some(rows.as_slice())
        } else {
            None
        }
    }

    /// Read-only access to the active row list. `None` when not
    /// `Active`.
    #[must_use]
    pub fn rows(&self) -> Option<&[HudEntry]> {
        if let Self::Active { rows, .. } = self {
            Some(rows.as_slice())
        } else {
            None
        }
    }
}

/// Enumerate keymap bindings whose first chord has *exactly* the
/// `held_mods` modifier set and whose `when` predicate (if any) holds
/// in `ctx`. The result is sorted by (namespace, chord label) so the
/// rendered two-column list groups by command namespace per §E6.
pub fn matching_bindings(
    keymap: &Keymap,
    held_mods: Modifiers,
    ctx: &dyn Context,
) -> Vec<HudEntry> {
    let mut out: Vec<HudEntry> = Vec::new();
    for b in &keymap.bindings {
        let Some(first) = b.keys.first() else {
            continue;
        };
        if first.modifiers != held_mods {
            continue;
        }
        // Predicate gate (parse on the fly is fine — the binding count
        // is small enough that this stays well below the 8 ms hot-path
        // budget; production wiring may cache parsed predicates).
        if let Some(expr) = b.when.as_deref() {
            let predicate = continuity_command::ContextPredicate::parse(expr);
            if !predicate.evaluate(ctx) {
                continue;
            }
        }
        let chord_label = render_chord_sequence(&b.keys);
        let namespace = b
            .command
            .split_once('.')
            .map(|(ns, _)| ns.to_string())
            .unwrap_or_default();
        out.push(HudEntry {
            command: b.command.clone(),
            chord_label,
            namespace,
        });
    }
    out.sort_by(|a, b| {
        a.namespace
            .cmp(&b.namespace)
            .then_with(|| a.chord_label.cmp(&b.chord_label))
            .then_with(|| a.command.cmp(&b.command))
    });
    out
}

/// Render a chord sequence as a `Ctrl+K, Ctrl+R`-style label.
fn render_chord_sequence(chords: &[continuity_input::KeyChord]) -> String {
    let parts: Vec<String> = chords.iter().map(render_chord).collect();
    parts.join(", ")
}

fn render_chord(c: &continuity_input::KeyChord) -> String {
    let mut s = String::new();
    if c.modifiers.ctrl {
        s.push_str("Ctrl+");
    }
    if c.modifiers.alt {
        s.push_str("Alt+");
    }
    if c.modifiers.shift {
        s.push_str("Shift+");
    }
    if c.modifiers.meta {
        s.push_str("Win+");
    }
    // Uppercase single-character keys (`f` → `F`) so the rendered label
    // matches the convention used in the keymap doc / HUD UX.
    if c.key.chars().count() == 1 {
        for ch in c.key.chars() {
            for up in ch.to_uppercase() {
                s.push(up);
            }
        }
    } else {
        s.push_str(&c.key);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn mods(c: bool, a: bool, s: bool) -> Modifiers {
        Modifiers {
            ctrl: c,
            alt: a,
            shift: s,
            meta: false,
        }
    }

    fn at_ms(ms: u64) -> TickMs {
        ms
    }

    /// Minimal Context stub: every predicate atom is considered true so
    /// every binding is applicable.
    struct PassCtx;
    impl continuity_command::ViewContext for PassCtx {}
    impl continuity_command::FindContext for PassCtx {}
    impl continuity_command::EditConfigContext for PassCtx {}

    impl Context for PassCtx {
        fn lookup(&self, _key: &str) -> Option<&str> {
            Some("true")
        }
        fn flag(&self, _key: &str) -> bool {
            true
        }
    }

    fn keymap(toml: &str) -> Keymap {
        Keymap::from_toml(toml).expect("test keymap parses")
    }

    #[test]
    fn idle_is_default_and_invisible() {
        let s = HudState::default();
        assert!(!s.is_visible());
        assert!(s.rows().is_none());
    }

    #[test]
    fn idle_to_pending_on_modifier_edge() {
        let mut s = HudState::default();
        s.on_modifier_edge(mods(true, false, false), at_ms(0));
        assert!(matches!(s, HudState::Pending { .. }));
        assert!(!s.is_visible());
    }

    #[test]
    fn modifier_release_returns_to_idle() {
        let mut s = HudState::default();
        s.on_modifier_edge(mods(true, false, false), at_ms(0));
        s.on_modifier_edge(mods(false, false, false), at_ms(100));
        assert!(matches!(s, HudState::Idle));
    }

    #[test]
    fn typed_chord_dismisses_pending() {
        let mut s = HudState::default();
        s.on_modifier_edge(mods(true, false, false), at_ms(0));
        s.on_chord_typed();
        assert!(matches!(s, HudState::Idle));
    }

    #[test]
    fn dwell_transition_to_active() {
        let mut s = HudState::default();
        s.on_modifier_edge(mods(true, false, false), at_ms(0));
        let km = keymap(
            r#"
            [[binding]]
            keys = ["ctrl+f"]
            command = "editor.find"

            [[binding]]
            keys = ["ctrl+shift+f"]
            command = "editor.find_all"
            "#,
        );
        let ctx = PassCtx;
        // Before dwell elapses → still pending.
        let before = s.poll(at_ms(HUD_DELAY_MS - 1), &km, &ctx);
        assert!(before.is_none());
        // Dwell elapsed → Active with one matching row.
        let rows = s.poll(at_ms(HUD_DELAY_MS), &km, &ctx).expect("rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].command, "editor.find");
        assert_eq!(rows[0].chord_label, "Ctrl+F");
        assert_eq!(rows[0].namespace, "editor");
        assert!(s.is_visible());
    }

    #[test]
    fn modifier_mask_change_resets_pending() {
        let mut s = HudState::default();
        s.on_modifier_edge(mods(true, false, false), at_ms(0));
        // User now presses Shift too.
        s.on_modifier_edge(mods(true, false, true), at_ms(100));
        match &s {
            HudState::Pending { since, .. } => {
                assert_eq!(*since, 100);
            }
            _ => panic!("expected fresh pending"),
        }
    }

    #[test]
    fn matching_bindings_filters_by_modifier_mask() {
        let km = keymap(
            r#"
            [[binding]]
            keys = ["ctrl+f"]
            command = "editor.find"

            [[binding]]
            keys = ["ctrl+shift+f"]
            command = "editor.find_all"

            [[binding]]
            keys = ["alt+1"]
            command = "view.heading_1"
            "#,
        );
        let ctx = PassCtx;
        let ctrl_only = matching_bindings(&km, mods(true, false, false), &ctx);
        assert_eq!(ctrl_only.len(), 1);
        assert_eq!(ctrl_only[0].command, "editor.find");

        let alt_only = matching_bindings(&km, mods(false, true, false), &ctx);
        assert_eq!(alt_only.len(), 1);
        assert_eq!(alt_only[0].command, "view.heading_1");

        let ctrl_shift = matching_bindings(&km, mods(true, false, true), &ctx);
        assert_eq!(ctrl_shift.len(), 1);
        assert_eq!(ctrl_shift[0].command, "editor.find_all");
    }

    #[test]
    fn matching_bindings_filters_by_predicate() {
        // A stub context with a single flag `editor.focused = true`.
        struct EditorCtx {
            allow: Cell<bool>,
        }
        impl continuity_command::ViewContext for EditorCtx {}
        impl continuity_command::FindContext for EditorCtx {}
        impl continuity_command::EditConfigContext for EditorCtx {}

        impl Context for EditorCtx {
            fn lookup(&self, key: &str) -> Option<&str> {
                if key == "editor.focused" {
                    Some(if self.allow.get() { "true" } else { "false" })
                } else {
                    None
                }
            }
            fn flag(&self, key: &str) -> bool {
                self.lookup(key) == Some("true")
            }
        }
        let km = keymap(
            r#"
            [[binding]]
            keys = ["ctrl+f"]
            command = "editor.find"
            when = "editor.focused"
            "#,
        );
        let ctx = EditorCtx {
            allow: Cell::new(true),
        };
        assert_eq!(
            matching_bindings(&km, mods(true, false, false), &ctx).len(),
            1
        );
        ctx.allow.set(false);
        assert_eq!(
            matching_bindings(&km, mods(true, false, false), &ctx).len(),
            0
        );
    }

    #[test]
    fn matching_bindings_groups_by_namespace_then_label() {
        let km = keymap(
            r#"
            [[binding]]
            keys = ["ctrl+t"]
            command = "view.toggle_minimap"

            [[binding]]
            keys = ["ctrl+a"]
            command = "editor.select_all"

            [[binding]]
            keys = ["ctrl+b"]
            command = "editor.bold"
            "#,
        );
        let ctx = PassCtx;
        let rows = matching_bindings(&km, mods(true, false, false), &ctx);
        // editor.* before view.* (namespace sort).
        let ns_order: Vec<_> = rows.iter().map(|r| r.namespace.clone()).collect();
        assert_eq!(ns_order, vec!["editor", "editor", "view"]);
        // Within editor.*, "Ctrl+A" before "Ctrl+B".
        let editor: Vec<_> = rows.iter().filter(|r| r.namespace == "editor").collect();
        assert_eq!(editor[0].chord_label, "Ctrl+A");
        assert_eq!(editor[1].chord_label, "Ctrl+B");
    }

    #[test]
    fn full_hold_flow_pending_then_active() {
        let mut s = HudState::default();
        let km = keymap(
            r#"
            [[binding]]
            keys = ["ctrl+f"]
            command = "editor.find"
            "#,
        );
        let ctx = PassCtx;
        // Hold Ctrl from t=0.
        s.on_modifier_edge(mods(true, false, false), at_ms(0));
        assert!(s.poll(at_ms(100), &km, &ctx).is_none());
        // 600 ms later → Active.
        assert!(s.poll(at_ms(600), &km, &ctx).is_some());
        // Release Ctrl → Idle.
        s.on_modifier_edge(mods(false, false, false), at_ms(700));
        assert!(!s.is_visible());
    }
}

// --------------------------------------------------------------------
// Integration follow-up site (§E6 scaffolding precedent of A1/A2):
//
// The state machine above is fully testable in isolation. The pieces
// that remain are pure wiring:
//
// 1. `Window::on_modifier_key` — pipe Win32 `WM_KEYDOWN` / `WM_KEYUP`
//    edges for `VK_CONTROL`, `VK_MENU`, `VK_SHIFT`, `VK_LWIN` /
//    `VK_RWIN` into [`HudState::on_modifier_edge`].
// 2. `Window::on_char` / chord dispatch — call
//    [`HudState::on_chord_typed`] when a non-modifier chord fires.
// 3. `Window::on_config_poll_tick` (or a dedicated 60-Hz HUD timer) —
//    call [`HudState::poll`] each tick; invalidate the window when a
//    `Some(rows)` return signals the panel just became visible.
// 4. `crates/ui/src/overlay_render.rs` — add a `layout_chord_hud` for
//    when `HudState::Active` is in flight; paint it on a layer that
//    sits above other overlays but does not consume input focus.
// --------------------------------------------------------------------
