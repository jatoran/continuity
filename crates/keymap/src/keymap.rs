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

    /// Bindings whose chord sequence exactly equals `pending` and whose
    /// `when` predicate holds, **ignoring** the longer-prefix rule that
    /// [`Self::match_sequence`] applies. Used by the chord dispatcher to
    /// fire a chord *leader* that doubles as a complete binding (e.g.
    /// `Ctrl+K` is both `markdown.insert_link` and the prefix of
    /// `Ctrl+K Ctrl+…`) when the leader is released without a
    /// continuation. Ordered last-wins-first, like
    /// [`Self::match_sequence_chain`].
    #[must_use]
    pub fn standalone_chain(&self, pending: &[KeyChord], ctx: &dyn Context) -> Vec<&Binding> {
        if pending.is_empty() {
            return Vec::new();
        }
        let in_ctx = |when: &Option<String>| {
            when.as_deref()
                .is_none_or(|w| ContextPredicate::parse(w).evaluate(ctx))
        };
        self.bindings
            .iter()
            .rev()
            .filter(|b| in_ctx(&b.when) && b.keys.as_slice() == pending)
            .collect()
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
mod tests;
