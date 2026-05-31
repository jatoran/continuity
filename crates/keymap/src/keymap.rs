//! `Keymap`: a parsed TOML keymap with conflict detection.

use continuity_command::{Context, ContextPredicate};
use continuity_input::KeyChord;
use serde::Deserialize;

use crate::{Binding, Conflict, Error};

/// A parsed, validated keymap.
#[derive(Debug, Default, Clone)]
pub struct Keymap {
    /// All bindings in source order.
    pub bindings: Vec<Binding>,
}

#[derive(Deserialize)]
struct RawKeymap {
    #[serde(default)]
    binding: Vec<RawBinding>,
}

#[derive(Deserialize)]
struct RawBinding {
    keys: Vec<String>,
    command: String,
    when: Option<String>,
}

impl Keymap {
    /// Parse a keymap from TOML.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Parse`] for malformed TOML or [`Error::Chord`] when
    /// any chord string is invalid.
    pub fn from_toml(s: &str) -> Result<Self, Error> {
        let raw: RawKeymap = toml::from_str(s)?;
        let mut bindings = Vec::with_capacity(raw.binding.len());
        for rb in raw.binding {
            let mut keys = Vec::with_capacity(rb.keys.len());
            for k in rb.keys {
                keys.push(k.parse::<KeyChord>()?);
            }
            bindings.push(Binding {
                keys,
                command: rb.command,
                when: rb.when,
            });
        }
        Ok(Self { bindings })
    }

    /// Return a new keymap with `overlay` bindings appended after `base`.
    ///
    /// Lookup walks bindings in reverse source order, so later overlay
    /// entries take precedence over defaults.
    #[must_use]
    pub fn layered(base: Self, overlay: Self) -> Self {
        let mut bindings = base.bindings;
        bindings.extend(overlay.bindings);
        Self { bindings }
    }

    /// Find the command bound to a single `chord` (legacy single-key
    /// lookup; equivalent to `match_sequence(&[chord], ctx)` returning
    /// `SequenceMatch::Match`).
    ///
    /// Later bindings win, which gives user keymaps ordinary overlay
    /// semantics.
    #[must_use]
    pub fn lookup(&self, chord: &KeyChord, ctx: &dyn Context) -> Option<&Binding> {
        match self.match_sequence(std::slice::from_ref(chord), ctx) {
            SequenceMatch::Match(b) => Some(b),
            SequenceMatch::Prefix | SequenceMatch::None => None,
        }
    }

    /// Match `pending` (the chords typed so far in the current
    /// chord-prefix window) against this keymap.
    ///
    /// Returns:
    /// - [`SequenceMatch::Match`] if `pending` exactly equals some
    ///   binding's `keys` sequence — caller should dispatch and reset
    ///   the pending buffer.
    /// - [`SequenceMatch::Prefix`] if `pending` is a strict prefix of
    ///   some binding's longer sequence — caller should keep the
    ///   pending buffer and wait for the next chord.
    /// - [`SequenceMatch::None`] if no binding matches — caller should
    ///   reset the pending buffer.
    ///
    /// When both a match and a longer-prefix exist (e.g. one binding is
    /// `[ctrl+k]` and another is `[ctrl+k, ctrl+r]`), the longer prefix
    /// wins so the user can complete the longer sequence.
    #[must_use]
    pub fn match_sequence(&self, pending: &[KeyChord], ctx: &dyn Context) -> SequenceMatch<'_> {
        if pending.is_empty() {
            return SequenceMatch::None;
        }
        let in_ctx = |when: &Option<String>| {
            when.as_deref()
                .is_none_or(|w| ContextPredicate::parse(w).evaluate(ctx))
        };
        let has_longer_prefix = self.bindings.iter().any(|b| {
            in_ctx(&b.when) && b.keys.len() > pending.len() && b.keys.starts_with(pending)
        });
        if has_longer_prefix {
            return SequenceMatch::Prefix;
        }
        let exact = self
            .bindings
            .iter()
            .rev()
            .find(|b| in_ctx(&b.when) && b.keys.as_slice() == pending);
        if let Some(b) = exact {
            return SequenceMatch::Match(b);
        }
        SequenceMatch::None
    }

    /// Same matching rules as [`Self::match_sequence`], but on a
    /// `Match` returns EVERY binding whose chord sequence equals
    /// `pending` and whose `when` predicate evaluates true, ordered
    /// last-wins-first (the entry [`Self::lookup`] picks comes first).
    ///
    /// The dispatcher uses this to retry when a high-priority
    /// handler returns `Error::UnsupportedContext`. The classic case
    /// is the cell-scoped table bindings: `markdown.table.move_up`
    /// only acts when the caret is in a body cell; outside that
    /// scope it returns `UnsupportedContext` and the dispatcher
    /// falls through to the next-most-specific binding (the global
    /// `editor.move_caret_up`). Without the chain the chord would
    /// go dead on table-edge motion.
    #[must_use]
    pub fn match_sequence_chain(
        &self,
        pending: &[KeyChord],
        ctx: &dyn Context,
    ) -> SequenceChainMatch<'_> {
        if pending.is_empty() {
            return SequenceChainMatch::None;
        }
        let in_ctx = |when: &Option<String>| {
            when.as_deref()
                .is_none_or(|w| ContextPredicate::parse(w).evaluate(ctx))
        };
        let has_longer_prefix = self.bindings.iter().any(|b| {
            in_ctx(&b.when) && b.keys.len() > pending.len() && b.keys.starts_with(pending)
        });
        if has_longer_prefix {
            return SequenceChainMatch::Prefix;
        }
        let matches: Vec<&Binding> = self
            .bindings
            .iter()
            .rev()
            .filter(|b| in_ctx(&b.when) && b.keys.as_slice() == pending)
            .collect();
        if matches.is_empty() {
            SequenceChainMatch::None
        } else {
            SequenceChainMatch::Match(matches)
        }
    }

    /// δ.2 — find the active binding for `command` so it can be
    /// surfaced in menu labels. Walks bindings in reverse source order
    /// (matching [`Self::lookup`]'s last-wins semantics), so user
    /// overlays beat defaults. Returns `None` when nothing is bound to
    /// `command` in this map.
    #[must_use]
    pub fn first_binding_for_command(&self, command: &str) -> Option<&Binding> {
        self.bindings.iter().rev().find(|b| b.command == command)
    }

    /// Find every conflict (two bindings with identical chord sequences
    /// under the same `when`).
    #[must_use]
    pub fn detect_conflicts(&self) -> Vec<Conflict> {
        let mut conflicts = Vec::new();
        for (i, a) in self.bindings.iter().enumerate() {
            for b in self.bindings.iter().skip(i + 1) {
                if a.when != b.when {
                    continue;
                }
                if a.keys == b.keys {
                    if let Some(k) = a.keys.last() {
                        conflicts.push(Conflict {
                            chord: k.clone(),
                            a: a.command.clone(),
                            b: b.command.clone(),
                            when: a.when.clone(),
                        });
                    }
                }
            }
        }
        conflicts
    }
}

/// Outcome of [`Keymap::match_sequence`].
#[derive(Debug, Clone)]
pub enum SequenceMatch<'a> {
    /// `pending` exactly matches a binding's chord sequence.
    Match(&'a Binding),
    /// `pending` is a strict prefix of one or more binding sequences.
    /// The dispatcher should hold the pending buffer and wait for the
    /// next chord.
    Prefix,
    /// No binding matches `pending`. Reset the pending buffer.
    None,
}

/// Like [`SequenceMatch`] but a successful match carries EVERY
/// binding that resolved, ordered from highest priority (the entry
/// [`Keymap::lookup`] would pick) to lowest. The dispatcher walks
/// the list in order, retrying after any binding whose handler
/// returns `Error::UnsupportedContext`, so a scoped no-op falls
/// through to the global default for the same chord.
#[derive(Debug)]
pub enum SequenceChainMatch<'a> {
    /// One or more bindings matched `pending`. Front of the `Vec` is
    /// the highest-priority binding; iterate front-to-back.
    Match(Vec<&'a Binding>),
    /// `pending` is a strict prefix of one or more binding sequences.
    Prefix,
    /// No binding matches `pending`.
    None,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[[binding]]
keys    = ["ctrl+alt+up"]
command = "editor.add_cursor_above"

[[binding]]
keys    = ["ctrl+b"]
command = "markdown.toggle_bold"
when    = "language == 'markdown'"

[[binding]]
keys    = ["ctrl+shift+k"]
command = "editor.delete_line"
"#;

    #[test]
    fn parses_sample_keymap() {
        let k = Keymap::from_toml(SAMPLE).unwrap();
        assert_eq!(k.bindings.len(), 3);
        assert_eq!(k.bindings[0].command, "editor.add_cursor_above");
        assert_eq!(
            k.bindings[1].when.as_deref(),
            Some("language == 'markdown'")
        );
        assert!(k.detect_conflicts().is_empty());
    }

    #[test]
    fn detects_conflicting_bindings() {
        let s = r#"
[[binding]]
keys    = ["ctrl+a"]
command = "first"

[[binding]]
keys    = ["ctrl+a"]
command = "second"
"#;
        let k = Keymap::from_toml(s).unwrap();
        let cs = k.detect_conflicts();
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].a, "first");
        assert_eq!(cs[0].b, "second");
    }

    #[test]
    fn ignores_conflicts_under_different_when() {
        let s = r#"
[[binding]]
keys    = ["ctrl+b"]
command = "global"

[[binding]]
keys    = ["ctrl+b"]
command = "markdown_only"
when    = "language == 'markdown'"
"#;
        let k = Keymap::from_toml(s).unwrap();
        assert!(k.detect_conflicts().is_empty());
    }

    #[test]
    fn rejects_invalid_chord_string() {
        let s = r#"
[[binding]]
keys    = ["ctrl++"]
command = "bad"
"#;
        assert!(Keymap::from_toml(s).is_err());
    }

    #[test]
    fn empty_keymap_ok() {
        let k = Keymap::from_toml("").unwrap();
        assert!(k.bindings.is_empty());
    }

    // --- Phase 17.6: chord-sequence semantics ----------------------------

    struct NullCtx;
    impl continuity_command::ViewContext for NullCtx {}
    impl continuity_command::FindContext for NullCtx {}
    impl Context for NullCtx {
        fn lookup(&self, _: &str) -> Option<&str> {
            None
        }
    }

    fn chord(s: &str) -> KeyChord {
        s.parse().unwrap()
    }

    #[test]
    fn single_chord_dispatches_immediately() {
        let s = r#"
[[binding]]
keys = ["ctrl+r"]
command = "editor.toggle_bullet"
"#;
        let k = Keymap::from_toml(s).unwrap();
        let pending = [chord("ctrl+r")];
        match k.match_sequence(&pending, &NullCtx) {
            SequenceMatch::Match(b) => assert_eq!(b.command, "editor.toggle_bullet"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn prefix_chord_waits_for_completion() {
        let s = r#"
[[binding]]
keys = ["ctrl+k", "ctrl+r"]
command = "theme.reload"
"#;
        let k = Keymap::from_toml(s).unwrap();
        match k.match_sequence(&[chord("ctrl+k")], &NullCtx) {
            SequenceMatch::Prefix => {}
            other => panic!("expected Prefix, got {other:?}"),
        }
        match k.match_sequence(&[chord("ctrl+k"), chord("ctrl+r")], &NullCtx) {
            SequenceMatch::Match(b) => assert_eq!(b.command, "theme.reload"),
            other => panic!("expected Match, got {other:?}"),
        }
    }

    #[test]
    fn unrelated_single_chord_does_not_collide_with_chord_prefix() {
        // `ctrl+k, ctrl+r` is a *two-key sequence* — `ctrl+r` alone must
        // not match it (otherwise the user can never bind `ctrl+r` to a
        // single-key command).
        let s = r#"
[[binding]]
keys = ["ctrl+r"]
command = "editor.toggle_bullet"

[[binding]]
keys = ["ctrl+k", "ctrl+r"]
command = "theme.reload"
"#;
        let k = Keymap::from_toml(s).unwrap();
        match k.match_sequence(&[chord("ctrl+r")], &NullCtx) {
            SequenceMatch::Match(b) => assert_eq!(b.command, "editor.toggle_bullet"),
            other => panic!("expected Match(toggle_bullet), got {other:?}"),
        }
    }

    #[test]
    fn no_match_returns_none() {
        let s = r#"
[[binding]]
keys = ["ctrl+r"]
command = "editor.toggle_bullet"
"#;
        let k = Keymap::from_toml(s).unwrap();
        match k.match_sequence(&[chord("ctrl+z")], &NullCtx) {
            SequenceMatch::None => {}
            other => panic!("expected None, got {other:?}"),
        }
    }

    #[test]
    fn lookup_prefers_later_overlay_binding() {
        struct Ctx;
        impl Context for Ctx {
            fn lookup(&self, key: &str) -> Option<&str> {
                match key {
                    "editor.focused" => Some("true"),
                    "language" => Some("plain"),
                    _ => None,
                }
            }
        }
        impl continuity_command::ViewContext for Ctx {}
        impl continuity_command::FindContext for Ctx {}

        let base = Keymap::from_toml(
            r#"
[[binding]]
keys = ["left"]
command = "base"
"#,
        )
        .unwrap();
        let overlay = Keymap::from_toml(
            r#"
[[binding]]
keys = ["left"]
command = "overlay"
"#,
        )
        .unwrap();
        let map = Keymap::layered(base, overlay);
        let chord: KeyChord = "left".parse().unwrap();
        assert_eq!(map.lookup(&chord, &Ctx).unwrap().command, "overlay");
    }

    #[test]
    fn lookup_respects_when_predicate() {
        struct Ctx;
        impl Context for Ctx {
            fn lookup(&self, key: &str) -> Option<&str> {
                match key {
                    "language" => Some("plain"),
                    _ => None,
                }
            }
        }
        impl continuity_command::ViewContext for Ctx {}
        impl continuity_command::FindContext for Ctx {}

        let map = Keymap::from_toml(
            r#"
[[binding]]
keys = ["ctrl+b"]
command = "markdown"
when = "language == 'markdown'"

[[binding]]
keys = ["ctrl+b"]
command = "plain"
when = "language == 'plain'"
"#,
        )
        .unwrap();
        let chord: KeyChord = "ctrl+b".parse().unwrap();
        assert_eq!(map.lookup(&chord, &Ctx).unwrap().command, "plain");
    }

    /// Phase F3 regression — `Ctrl+Alt+H` must resolve to
    /// `markdown.highlight_selection` when the buffer's language atom is
    /// `markdown`. A user-reported bug claimed the chord did nothing in
    /// production; pin both the parsed and the runtime chord shapes
    /// against the bundled keymap so future binding edits (or chord-
    /// parser regressions) don't quietly drop the wire.
    #[test]
    fn default_keymap_binds_ctrl_alt_h_to_markdown_highlight_selection() {
        use continuity_input::Modifiers;
        struct MarkdownCtx;
        impl continuity_command::ViewContext for MarkdownCtx {}
        impl continuity_command::FindContext for MarkdownCtx {}
        impl Context for MarkdownCtx {
            fn lookup(&self, key: &str) -> Option<&str> {
                match key {
                    "language" => Some("markdown"),
                    "editor.focused" => Some("true"),
                    _ => None,
                }
            }
        }
        let map = Keymap::from_toml(crate::DEFAULT_KEYMAP_TOML).expect("bundled keymap parses");
        let parsed: KeyChord = "ctrl+alt+h".parse().expect("ctrl+alt+h parses");
        assert_eq!(
            map.lookup(&parsed, &MarkdownCtx)
                .map(|b| b.command.as_str()),
            Some("markdown.highlight_selection"),
            "parsed `ctrl+alt+h` under markdown ctx must resolve to markdown.highlight_selection"
        );
        // Runtime path: 'H' is virtual-key 0x48; chord built with ctrl + alt
        // must match the parsed chord byte-for-byte.
        let runtime = KeyChord::from_vk_modifiers(
            0x48,
            Modifiers {
                ctrl: true,
                alt: true,
                ..Modifiers::default()
            },
        )
        .expect("VK 0x48 maps to a key name");
        assert_eq!(
            parsed, runtime,
            "runtime ctrl+alt+h must equal parsed chord"
        );
        assert_eq!(
            map.lookup(&runtime, &MarkdownCtx)
                .map(|b| b.command.as_str()),
            Some("markdown.highlight_selection"),
            "runtime ctrl+alt+h under markdown ctx must resolve to markdown.highlight_selection"
        );
        // Negative: same chord under a non-markdown language must NOT
        // fire the binding (the `when` predicate gates it).
        struct PlainCtx;
        impl continuity_command::ViewContext for PlainCtx {}
        impl continuity_command::FindContext for PlainCtx {}
        impl Context for PlainCtx {
            fn lookup(&self, key: &str) -> Option<&str> {
                match key {
                    "language" => Some("plain"),
                    "editor.focused" => Some("true"),
                    _ => None,
                }
            }
        }
        assert_eq!(
            map.lookup(&parsed, &PlainCtx).map(|b| b.command.as_str()),
            None,
            "ctrl+alt+h must NOT fire markdown.highlight_selection when language is plain"
        );
    }

    /// Phase F3 — `Ctrl+Alt+Shift+H` resolves to
    /// `markdown.clear_inline_color` (the unwrap chord).
    #[test]
    fn default_keymap_binds_ctrl_alt_shift_h_to_clear_inline_color() {
        struct MarkdownCtx;
        impl continuity_command::ViewContext for MarkdownCtx {}
        impl continuity_command::FindContext for MarkdownCtx {}
        impl Context for MarkdownCtx {
            fn lookup(&self, key: &str) -> Option<&str> {
                match key {
                    "language" => Some("markdown"),
                    "editor.focused" => Some("true"),
                    _ => None,
                }
            }
        }
        let map = Keymap::from_toml(crate::DEFAULT_KEYMAP_TOML).expect("bundled keymap parses");
        let parsed: KeyChord = "ctrl+alt+shift+h".parse().expect("chord parses");
        assert_eq!(
            map.lookup(&parsed, &MarkdownCtx)
                .map(|b| b.command.as_str()),
            Some("markdown.clear_inline_color"),
        );
    }

    #[test]
    fn default_keymap_binds_ctrl_slash_to_slash_palette_show() {
        use continuity_input::Modifiers;
        // §H5 — the bundled keymap binds `Ctrl+/` to
        // `view.slash_palette_show`. This test pins the binding via
        // both the parser shape (`"ctrl+/".parse()`) AND the runtime
        // shape (a chord built with VK_OEM_2 + Ctrl modifier) so a
        // regression in either path is caught immediately. Before
        // VK_OEM_2 → "/" mapping landed, the chord built at runtime
        // produced `None` and Ctrl+/ never reached the registry.
        let map = Keymap::from_toml(crate::DEFAULT_KEYMAP_TOML).expect("bundled keymap parses");
        // Path 1: chord parsed from the chord-grammar string.
        let parsed: KeyChord = "ctrl+/".parse().expect("ctrl+/ parses");
        assert_eq!(
            map.lookup(&parsed, &NullCtx).map(|b| b.command.as_str()),
            Some("view.slash_palette_show"),
            "parsed `ctrl+/` should resolve to view.slash_palette_show"
        );
        // Path 2: chord built from the runtime VK + modifiers (the
        // shape `Window::on_keydown` produces). 0xBF is VK_OEM_2.
        let runtime = KeyChord::from_vk_modifiers(
            0xBF,
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
        )
        .expect("VK_OEM_2 maps to a key name");
        assert_eq!(parsed, runtime, "runtime chord must match parsed chord");
        assert_eq!(
            map.lookup(&runtime, &NullCtx).map(|b| b.command.as_str()),
            Some("view.slash_palette_show"),
            "runtime `ctrl+/` (VK_OEM_2 + ctrl) should resolve to view.slash_palette_show"
        );
    }
}
