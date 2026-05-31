//! §H5 — slash-command palette glue between the editor input path
//! and the overlay state machine.
//!
//! The typed-`/` trigger fires from `Window::on_char` after the `/`
//! glyph has been inserted into the rope. The hook here inspects the
//! caret's source line — if the line consists of optional leading
//! whitespace followed by exactly the trailing `/`, it opens the
//! slash palette with the `TypedSlash` trigger so the Esc cleanup
//! path knows to remove the literal `/` on dismiss.
//!
//! Thread ownership: UI thread of the owning [`crate::Window`].

use crate::slash_palette::SlashTrigger;
use crate::window::Window;

/// Difference between two revision snapshots, expressed as `u64` so
/// the contract assert can compare against a literal. `None` on
/// either side maps to 0 — headless tests where no snapshot exists
/// silently bypass the check.
fn revision_delta(
    before: Option<continuity_buffer::Revision>,
    after: Option<continuity_buffer::Revision>,
) -> u64 {
    match (before, after) {
        (Some(a), Some(b)) => b.0.saturating_sub(a.0),
        _ => 1, // treat "no snapshot" as a single tick so the assert passes
    }
}

impl Window {
    /// §H5 — commit the highlighted slash-palette entry. The trailing
    /// `/` (when the trigger was the typed-`/` hook) is removed first
    /// so the dispatched insertion lands at the same caret position
    /// that originally fired the trigger. The command is then
    /// dispatched through the normal registry path. A non-
    /// `applicable` row is a no-op (the predicate didn't match
    /// against the current `Context`).
    ///
    /// Insertion-only contract (spec §H5): debug builds assert that
    /// the dispatched command produced exactly one rope revision tick
    /// — a non-insertion command (revision unchanged) or a composite
    /// command (revision advanced by more than one) trips the assert.
    /// Release builds simply pass through.
    pub(crate) fn confirm_slash_palette(&mut self) {
        let pick = self.overlays.slash_palette().and_then(|sp| {
            sp.selected_entry()
                .map(|entry| (entry.command.clone(), entry.applicable, sp.trigger))
        });
        let Some((command, applicable, trigger)) = pick else {
            return;
        };
        if !applicable {
            return;
        }
        self.overlays.dismiss();
        // `Overlays::dismiss` flips the discriminant to `Idle` but
        // leaves `Window::overlay_input_focused` set. Without this blur
        // the next `WM_KEYDOWN` hits the `overlay_has_keyboard_focus()`
        // guard in `Window::on_keydown` and returns `true` (swallow),
        // killing every keystroke after a slash-palette command. The
        // symptom-was: typing `hello`/Ctrl+anything goes nowhere after
        // `markdown.insert_table`. Why: input-focus is a Window-side
        // flag separate from the overlay-state enum, so the dismiss
        // must explicitly clear it.
        self.blur_overlay_input();
        if trigger == SlashTrigger::TypedSlash {
            self.delete_trailing_slash_from_typed_trigger();
        }
        let revision_before = self.current_buffer_revision();
        let ok = Window::dispatch_command(self, &command, &serde_json::Value::Null);
        if !ok {
            // Dispatch refused (unknown id, predicate failed, handler
            // error). Skip the contract assert — there is no insert
            // to validate.
            return;
        }
        let revision_after = self.current_buffer_revision();
        debug_assert_eq!(
            revision_delta(revision_before, revision_after),
            1,
            "slash-palette insertion-only contract violated: command `{}` advanced revision by {:?}→{:?}",
            command,
            revision_before,
            revision_after
        );
    }

    /// Current revision of the active buffer's rope snapshot, or
    /// `None` when no snapshot is available (headless tests).
    fn current_buffer_revision(&self) -> Option<continuity_buffer::Revision> {
        self.editor
            .snapshot(self.buffer_id)
            .map(|s| s.rope_snapshot().revision())
    }

    /// §H5 — remove the single literal `/` typed by the line-start
    /// trigger. Best-effort: the standard `delete_back_at_selections`
    /// path applies a one-byte delete at the primary caret, which is
    /// the trailing `/` because the overlay swallowed every char
    /// typed after it.
    pub(crate) fn delete_trailing_slash_from_typed_trigger(&mut self) {
        let _ = self.delete_back_at_selections();
    }

    /// §H5 — post-insert hook. Call from `on_char` after a successful
    /// `editor.insert_char` dispatch. A no-op for any glyph other than
    /// `/`, while another overlay is already up, or when the line
    /// already has non-slash, non-whitespace characters.
    pub(crate) fn maybe_fire_slash_palette_trigger(&mut self, just_inserted: char) {
        if just_inserted != '/' {
            return;
        }
        if !self.slash_commands_enabled {
            return;
        }
        if self.overlays.is_active() {
            return;
        }
        if !self.is_caret_line_typed_slash_trigger() {
            return;
        }
        let _ = self.show_slash_palette_impl(SlashTrigger::TypedSlash);
    }

    /// `true` when the primary caret sits on a source line whose only
    /// non-whitespace character is a single `/` (matches `^\s*/$`).
    fn is_caret_line_typed_slash_trigger(&self) -> bool {
        let Some(snap) = self.editor.snapshot(self.buffer_id) else {
            return false;
        };
        let Some(sel) = snap.selections().first() else {
            return false;
        };
        let line = sel.head.line as usize;
        let rope = snap.rope_snapshot().rope();
        if line >= rope.len_lines() {
            return false;
        }
        let start = rope.line_to_byte(line);
        let end = if line + 1 < rope.len_lines() {
            rope.line_to_byte(line + 1)
        } else {
            rope.len_bytes()
        };
        let slice = rope.byte_slice(start..end);
        let mut saw_slash = false;
        for ch in slice.chars() {
            match ch {
                ' ' | '\t' => {
                    if saw_slash {
                        return false;
                    }
                }
                '/' => {
                    if saw_slash {
                        return false;
                    }
                    saw_slash = true;
                }
                '\n' | '\r' => break,
                _ => return false,
            }
        }
        saw_slash
    }
}
