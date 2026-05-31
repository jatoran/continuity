//! §H5 — slash-command palette state.
//!
//! Triggered by typing `/` as the first non-whitespace character of a
//! line (any buffer; not markdown-only), and by the explicit
//! `Ctrl+/` chord that routes through `view.slash_palette_show`.
//! Lists only the command-registry's `palette_safe` insertion safelist;
//! destructive commands are filtered out structurally by the A7
//! `palette_safe` predicate.
//!
//! Thread ownership: UI thread of the owning [`crate::Window`]. State
//! is created by `show_slash_palette_impl`, mutated by the overlay
//! step / confirm / cancel routes in `window_overlays.rs`, and dropped
//! by `Overlays::dismiss`.

use continuity_search::{score, FuzzyMatch};

use crate::text_input::TextInput;

/// One slash-palette candidate.
#[derive(Clone, Debug)]
pub struct SlashPaletteEntry {
    /// Command name (`markdown.insert_table`, …).
    pub command: String,
    /// Display label (defaults to the command name; richer registry
    /// metadata can fill this once it lands).
    pub label: String,
    /// Optional short description (one-line, palette hint).
    pub description: Option<String>,
    /// Optional displayed binding hint (`Ctrl+Alt+T`, …).
    pub keybinding: Option<String>,
    /// `false` when the active context predicate didn't apply; the row
    /// is rendered grayed-out and Enter is suppressed.
    pub applicable: bool,
}

/// Trigger origin for the slash palette — affects the Esc dismiss path
/// (typed-`/` removes the trailing slash, explicit chord does not).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum SlashTrigger {
    /// Opened by typing `/` at the start of a line. Esc must remove
    /// the literal `/` from the rope; Backspace before any filter
    /// chars typed dismisses but leaves the `/` in source.
    #[default]
    TypedSlash,
    /// Opened via the `view.slash_palette_show` chord (`Ctrl+/`). No
    /// trailing slash to clean up — Esc just dismisses.
    ExplicitChord,
}

/// Slash-palette state.
#[derive(Debug, Default)]
pub struct SlashPalette {
    /// Filter input — characters typed after the `/`.
    pub input: TextInput,
    /// Full safelist, captured at open time.
    pub all: Vec<SlashPaletteEntry>,
    /// Indices into `all` of currently-shown matches, in score order.
    pub filtered: Vec<usize>,
    /// Per-filtered-row fuzzy-match metadata (scored against `label`).
    pub matches: Vec<FuzzyMatch>,
    /// Selected row within `filtered`.
    pub selected: usize,
    /// `true` once the user has typed at least one filter character.
    /// Backspace at `false` dismisses the palette; once `true`, the
    /// usual text-input delete path applies.
    pub has_filter_chars: bool,
    /// Source-line (zero-indexed) where the trigger fired. Used by the
    /// painter to dock the popup near the caret and by the Esc-cleanup
    /// path to address the trailing `/` for removal.
    pub anchor_line: u32,
    /// `true` when the trigger was a typed `/` (vs the explicit chord);
    /// gates the Esc trailing-slash cleanup.
    pub trigger: SlashTrigger,
}

impl SlashPalette {
    /// Build a new palette anchored at `anchor_line` with `trigger`
    /// recording how it was opened.
    #[must_use]
    pub fn new(entries: Vec<SlashPaletteEntry>, anchor_line: u32, trigger: SlashTrigger) -> Self {
        let mut palette = Self {
            input: TextInput::default(),
            all: entries,
            filtered: Vec::new(),
            matches: Vec::new(),
            selected: 0,
            has_filter_chars: false,
            anchor_line,
            trigger,
        };
        palette.refilter();
        palette
    }

    /// Re-rank against the current filter. Empty filter shows every
    /// candidate in registry order — the safelist is short enough
    /// (~10 commands) that listing all of them is the right UX.
    pub(crate) fn refilter(&mut self) {
        let q = self.input.text.as_str();
        if q.is_empty() {
            self.filtered = (0..self.all.len()).collect();
            self.matches.clear();
            self.selected = self.selected.min(self.filtered.len().saturating_sub(1));
            return;
        }
        let mut scored: Vec<(usize, FuzzyMatch)> = self
            .all
            .iter()
            .enumerate()
            .filter_map(|(i, entry)| score(q, &entry.label).map(|m| (i, m)))
            .collect();
        scored.sort_by(|a, b| {
            b.1.score
                .cmp(&a.1.score)
                .then_with(|| self.all[a.0].label.cmp(&self.all[b.0].label))
        });
        self.filtered.clear();
        self.matches.clear();
        for (i, m) in scored {
            self.filtered.push(i);
            self.matches.push(m);
        }
        self.selected = self.selected.min(self.filtered.len().saturating_sub(1));
    }

    /// Move the selection cursor by `delta` rows.
    pub fn step(&mut self, delta: i32) {
        if self.filtered.is_empty() {
            self.selected = 0;
            return;
        }
        let len = self.filtered.len() as i32;
        let next = (self.selected as i32 + delta).max(0).min(len - 1);
        self.selected = next as usize;
    }

    /// Currently-selected entry.
    #[must_use]
    pub fn selected_entry(&self) -> Option<&SlashPaletteEntry> {
        self.filtered
            .get(self.selected)
            .and_then(|i| self.all.get(*i))
    }

    /// Note that a filter character was just typed. Promotes the
    /// palette out of the "Backspace dismisses" zero-typed state.
    pub fn note_filter_char(&mut self) {
        self.has_filter_chars = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(command: &str, label: &str) -> SlashPaletteEntry {
        SlashPaletteEntry {
            command: command.into(),
            label: label.into(),
            description: None,
            keybinding: None,
            applicable: true,
        }
    }

    #[test]
    fn empty_filter_lists_every_candidate_in_registration_order() {
        let p = SlashPalette::new(
            vec![entry("a.x", "alpha"), entry("b.y", "beta")],
            0,
            SlashTrigger::TypedSlash,
        );
        assert_eq!(p.filtered.len(), 2);
        assert_eq!(p.filtered, vec![0, 1]);
    }

    #[test]
    fn typed_filter_reranks_by_match() {
        let mut p = SlashPalette::new(
            vec![
                entry("a.x", "alpha"),
                entry("b.y", "beta"),
                entry("c.z", "gamma"),
            ],
            0,
            SlashTrigger::TypedSlash,
        );
        p.input.set_text("be");
        p.refilter();
        assert_eq!(p.selected_entry().unwrap().label, "beta");
    }

    #[test]
    fn step_clamps_in_both_directions() {
        let mut p = SlashPalette::new(
            vec![entry("a", "alpha"), entry("b", "beta")],
            0,
            SlashTrigger::TypedSlash,
        );
        p.step(-3);
        assert_eq!(p.selected, 0);
        p.step(50);
        assert_eq!(p.selected, 1);
    }

    #[test]
    fn note_filter_char_promotes_state() {
        let mut p = SlashPalette::new(vec![entry("a", "alpha")], 0, SlashTrigger::TypedSlash);
        assert!(!p.has_filter_chars);
        p.note_filter_char();
        assert!(p.has_filter_chars);
    }

    #[test]
    fn anchor_line_and_trigger_round_trip() {
        let p = SlashPalette::new(vec![], 42, SlashTrigger::ExplicitChord);
        assert_eq!(p.anchor_line, 42);
        assert_eq!(p.trigger, SlashTrigger::ExplicitChord);
    }
}
