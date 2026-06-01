//! Command dispatch and keymap handling for [`Window`].
//!
//! The `impl Context for Window` block lives in the sibling
//! [`context`] submodule so this file stays under the 600-line cap.

mod context;

use continuity_command::{EDITOR_INSERT_CHAR, TAB_CLOSE};
use continuity_input::KeyChord;
use serde_json::Value;

use crate::window_input_modifiers::active_modifiers;
use crate::Window;

/// Tristate result of a single command dispatch. Drives the chord
/// loop's decision to retry against the next-most-specific binding
/// when a scoped handler is inapplicable. See
/// [`Window::dispatch_command_outcome`] for the semantics of each
/// variant.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum DispatchOutcome {
    /// Handler ran to completion (`Ok(())`).
    Handled,
    /// Handler declined the context (`UnsupportedContext`). Chord
    /// dispatcher should fall through to the next binding.
    Unsupported,
    /// Registry didn't resolve the command. Same fall-through
    /// semantics as `Unsupported`.
    Skipped,
    /// Handler returned a hard error. Chord is consumed; no
    /// fall-through (the binding is real and the failure is real).
    Failed,
}

/// Compact token describing the JSON shape of the args payload on a
/// dispatch. Surfaced on `event:command_dispatch args=` so the
/// analyzer can separate keybind chords (typically `null`) from
/// palette / drop / clipboard dispatches that carry payloads.
fn args_kind_token(args: &Value) -> &'static str {
    match args {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

impl Window {
    pub(crate) fn on_char(&mut self, code: u32) -> bool {
        // Filter every control character. `Enter`, `Backspace`, `Delete`,
        // and `Tab` are dispatched through the keymap on `WM_KEYDOWN`; if
        // we let CR (0x0d) / LF (0x0a) / HT (0x09) reach
        // `editor.insert_char` here as well, the newline lands twice (once
        // as the bound `editor.insert_newline` command, once as the raw
        // character) — that was the "Enter skips a line" bug.
        if code < 0x20 {
            return false;
        }
        if code == 0x7f {
            return false; // DEL handled in keydown
        }
        let Some(ch) = char::from_u32(code) else {
            return false;
        };
        let hud_dirty = self.on_chord_hud_typed();
        self.note_input_now();
        if self.overlay_has_keyboard_focus() {
            return self.overlay_on_char(ch);
        }
        // δ.1 — surround-on-type. When a paired open character is
        // typed against a non-empty selection AND auto-pair is enabled
        // for that pair, wrap the selection instead of replacing it
        // with the typed char. Runs before insert_char dispatch so the
        // selection's source bytes survive. `auto_pair_for` already
        // honours the per-char `auto_pair_*` settings (asterisk and
        // underscore default off so emphasis prose stays well-behaved).
        if self.try_surround_on_type(ch) {
            self.note_metrics_keystroke(crate::window_time_machine::MetricsKeystroke::Inserted {
                chars: 1,
            });
            return true;
        }
        let dispatched =
            self.dispatch_command(EDITOR_INSERT_CHAR.as_str(), &Value::String(ch.to_string()));
        if dispatched {
            self.note_metrics_keystroke(crate::window_time_machine::MetricsKeystroke::Inserted {
                chars: 1,
            });
            // §H5 — line-start `/` opens the slash-command palette.
            self.maybe_fire_slash_palette_trigger(ch);
        }
        dispatched || hud_dirty
    }

    pub(crate) fn on_keydown(&mut self, vk: u16) -> bool {
        let modifiers = active_modifiers();
        self.shift_held = modifiers.shift;
        self.note_input_now();
        let hover_cleared = if vk == windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE.0 {
            false
        } else {
            self.clear_footnote_hover()
        };
        let Some(chord) = KeyChord::from_vk_modifiers(vk, modifiers) else {
            // Modifier-only press: don't disturb a pending chord sequence.
            self.on_chord_hud_modifier_edge(modifiers);
            return hover_cleared;
        };
        let hud_dirty = self.on_chord_hud_typed();
        if self.overlay_has_keyboard_focus() {
            self.pending_chord_sequence.clear();
            if self.overlay_dispatch_find_mode_chord(&chord) {
                return true;
            }
            // Text-editing chords (Ctrl+A/C/X/V, Shift+Home/End/arrows)
            // win over the overlay's own routing so they act on the
            // focused input instead of leaking to the editor behind it.
            if self.overlay_intercept_text_chord(vk, &chord) {
                return true;
            }
            if self.overlay_on_keydown(vk, &chord) {
                return true;
            }
            // While an overlay is active, anything the overlay didn't
            // claim is swallowed. Falling through to `keymap.lookup`
            // would dispatch buffer-scoped commands like
            // `editor.select_all` against the underlying document
            // while the user is typing into the palette — the chord
            // belongs to the overlay until it closes.
            return true;
        }
        // Phase-I1 — when the time-machine slider is open it owns
        // Enter (commit) and Esc (cancel). Run before the universal
        // dismiss chain so Esc cancels the slider rather than the
        // banner / hover / etc. behind it.
        if self.view_options.time_machine.timeline_visible && self.handle_time_machine_keystroke(vk)
        {
            return true;
        }
        // Buffer-history tab: arrows / PgUp / PgDn / Home / End move
        // the selected lane; Enter adopts the buffer; Esc closes the
        // tab. Runs early so the regular keymap lookup doesn't
        // dispatch buffer-scoped commands against the synthetic
        // empty buffer behind the panel.
        if self.try_buffer_history_keystroke(vk) {
            return true;
        }
        // Phase B3 Esc universal-dismiss priority chain. Banners /
        // future view-overlay previews / etc. get first crack at Esc;
        // if nothing consumes it we fall through to the keymap, which
        // binds Esc → editor.clear_secondary_cursors by default.
        if vk == windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE.0
            && self.dismiss_priority_chain()
        {
            return true;
        }
        // Build the tentative sequence (previous pending + this chord).
        let mut seq = std::mem::take(&mut self.pending_chord_sequence);
        seq.push(chord.clone());
        let command_chain = match self.keymap.match_sequence_chain(&seq, self) {
            continuity_keymap::SequenceChainMatch::Match(bindings) => Some(
                bindings
                    .into_iter()
                    .map(|b| b.command.clone())
                    .collect::<Vec<String>>(),
            ),
            continuity_keymap::SequenceChainMatch::Prefix => {
                // Hold the pending sequence and wait for the next chord.
                self.pending_chord_sequence = seq;
                return true;
            }
            continuity_keymap::SequenceChainMatch::None => None,
        };
        if let Some(chain) = command_chain {
            let consumed = self.dispatch_chord_chain(&chain);
            return consumed || hud_dirty || hover_cleared;
        }
        // The accumulated sequence didn't pan out. If we were
        // mid-sequence, try this chord *fresh* — that way a stray
        // Ctrl+K followed by Ctrl+S still saves the file rather
        // than swallowing both keys.
        if seq.len() > 1 {
            let single_chain = match self
                .keymap
                .match_sequence_chain(std::slice::from_ref(&chord), self)
            {
                continuity_keymap::SequenceChainMatch::Match(bindings) => Some(
                    bindings
                        .into_iter()
                        .map(|b| b.command.clone())
                        .collect::<Vec<String>>(),
                ),
                _ => None,
            };
            if let Some(chain) = single_chain {
                let consumed = self.dispatch_chord_chain(&chain);
                return consumed || hud_dirty || hover_cleared;
            }
        }
        hud_dirty || hover_cleared
    }

    /// Walk a chord's matching command ids in priority order. Each
    /// command gets one dispatch; on `Unsupported` (handler declined)
    /// or `Skipped` (handler missing) the loop continues to the next
    /// command. On `Handled` or `Failed` the loop stops — the chord
    /// is fully consumed in either case. Returns `true` when any
    /// command consumed the chord (i.e. anything except a chain
    /// where every binding declined / was missing).
    fn dispatch_chord_chain(&mut self, commands: &[String]) -> bool {
        for command in commands {
            match self.dispatch_command_outcome(command, &Value::Null) {
                DispatchOutcome::Handled | DispatchOutcome::Failed => return true,
                DispatchOutcome::Unsupported | DispatchOutcome::Skipped => continue,
            }
        }
        false
    }

    /// Fire a chord *leader*'s standalone binding when Ctrl is released
    /// without a continuation. `Ctrl+K` is both `markdown.insert_link`
    /// and the prefix of the `Ctrl+K …` chords, so [`Self::on_keydown`]
    /// holds it as a pending prefix (waiting for a second key); releasing
    /// Ctrl with the leader still pending dispatches its standalone
    /// binding here. Returns `true` when a command consumed the chord.
    pub(crate) fn flush_pending_chord_standalone(&mut self) -> bool {
        if self.pending_chord_sequence.is_empty() {
            return false;
        }
        if self.overlay_has_keyboard_focus() {
            self.pending_chord_sequence.clear();
            return false;
        }
        let pending = std::mem::take(&mut self.pending_chord_sequence);
        let chain: Vec<String> = self
            .keymap
            .standalone_chain(&pending, self)
            .into_iter()
            .map(|b| b.command.clone())
            .collect();
        if chain.is_empty() {
            return false;
        }
        self.dispatch_chord_chain(&chain)
    }

    pub(crate) fn refresh_keymap_conflicts(&mut self) {
        self.keymap_conflicts = self.keymap.detect_conflicts();
    }
    // δ.1 — surround-on-type lives in `window_surround_on_type.rs`.

    pub(crate) fn dispatch_command(&mut self, command: &str, args: &Value) -> bool {
        !matches!(
            self.dispatch_command_outcome(command, args),
            DispatchOutcome::Skipped | DispatchOutcome::Unsupported
        )
    }

    /// Dispatch one command and report whether the chord should be
    /// considered "handled" or whether the keymap layer should fall
    /// through to the next-most-specific binding for the same chord.
    ///
    /// `Handled` — the handler ran to completion (`Ok(())`) and the
    /// chord is fully consumed.
    /// `Unsupported` — the handler explicitly declared the chord
    /// inapplicable in the current context (`UnsupportedContext`).
    /// The chord dispatcher should retry with the next binding so a
    /// scoped no-op gracefully falls through to the global default.
    /// `Skipped` — the registry rejected the command id (unknown /
    /// not applicable). Same fall-through semantics as `Unsupported`.
    /// `Failed` — handler ran but returned a hard error. The chord
    /// is consumed; no fall-through (retrying after a real failure
    /// would compound the user-visible misbehavior).
    pub(crate) fn dispatch_command_outcome(
        &mut self,
        command: &str,
        args: &Value,
    ) -> DispatchOutcome {
        if command != TAB_CLOSE.as_str() {
            self.clear_unsaved_close_arm();
        }
        // `event:command_dispatch` mirror. The `Registry::dispatch`
        // wrapper in `crates/command/src/registry.rs` instruments
        // command-dispatch flow, but this site uses
        // `handler_for_name` + direct invocation so the wrapper would
        // be silently bypassed. Emit the same shape here so keybind
        // and palette presses surface in the trace.
        let trace_started = continuity_trace::is_enabled().then(std::time::Instant::now);
        let handler = match self.registry.handler_for_name(command, self) {
            Ok(handler) => handler,
            Err(e) => {
                if let Some(started) = trace_started {
                    let dur_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
                    let outcome_token = match &e {
                        continuity_command::Error::UnknownCommand(_) => "unknown_command",
                        continuity_command::Error::NotApplicable(_) => "not_applicable",
                        continuity_command::Error::UnsupportedContext(_) => "unsupported_context",
                        _ => "err",
                    };
                    continuity_trace::log_event_us(
                        "command_dispatch",
                        dur_us,
                        &format!(
                            "id={command} outcome={outcome_token} args={} source=window",
                            args_kind_token(args),
                        ),
                    );
                }
                eprintln!("command `{command}` not dispatched: {e}");
                return DispatchOutcome::Skipped;
            }
        };
        let dispatch_result = handler(args, self);
        let outcome = match &dispatch_result {
            Ok(()) => DispatchOutcome::Handled,
            Err(continuity_command::Error::UnsupportedContext(_)) => DispatchOutcome::Unsupported,
            Err(_) => DispatchOutcome::Failed,
        };
        let ok = matches!(outcome, DispatchOutcome::Handled);
        if let Err(e) = &dispatch_result {
            if !matches!(outcome, DispatchOutcome::Unsupported) {
                eprintln!("command `{command}` failed: {e}");
            }
        }
        if let Some(started) = trace_started {
            let dur_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
            let outcome_token = match &dispatch_result {
                Ok(()) => "ok",
                Err(continuity_command::Error::UnknownCommand(_)) => "unknown_command",
                Err(continuity_command::Error::NotApplicable(_)) => "not_applicable",
                Err(continuity_command::Error::UnsupportedContext(_)) => "unsupported_context",
                Err(_) => "err",
            };
            continuity_trace::log_event_us(
                "command_dispatch",
                dur_us,
                &format!(
                    "id={command} outcome={outcome_token} args={} source=window",
                    args_kind_token(args),
                ),
            );
        }
        // After successful caret/edit motion the viewport follows the
        // primary caret so typing past the bottom auto-scrolls. Skip for
        // explicit view-scroll commands (the user just moved the viewport;
        // don't snap it back to the caret). Also skip the document-end
        // motions: their viewport follow is owned by the deferred
        // exact-bottom snap (`pending_doc_end_scroll`), and the generic
        // reveal — which resolves the caret's display row from a possibly
        // stale/partial projection — would otherwise under-scroll first
        // and fight the snap on a soft-wrapped buffer.
        // `editor.select_all` must not move the viewport: selecting the
        // whole buffer leaves the primary caret at the document end, so the
        // generic reveal would scroll to the bottom. The user expects the
        // view to stay put while everything highlights.
        let scroll_owned_by_doc_end_snap =
            command == "editor.move_doc_end" || command == "editor.extend_doc_end";
        let select_all_keeps_view = command == "editor.select_all";
        if ok
            && !command.starts_with("view.scroll_")
            && !scroll_owned_by_doc_end_snap
            && !select_all_keeps_view
        {
            self.ensure_primary_caret_visible();
        }
        if ok {
            // Phase-10 source↔display motion-skip: after a basic char move
            // the head may have landed inside a hidden structural marker
            // range — advance past it in the direction of travel.
            let skip_dir = match command {
                "editor.move_char_forward" | "editor.extend_char_forward" => 1,
                "editor.move_char_backward" | "editor.extend_char_backward" => -1,
                _ => 0,
            };
            if skip_dir != 0 {
                self.apply_structural_skip(skip_dir);
            }
            self.refresh_language();
            self.maybe_submit_decoration();
        }
        outcome
    }

    // `log_keymap_conflicts` and `reload_keymap_from_sources` moved
    // to `window_keymap_reload.rs` (Phase H6 split, 2026-05-13) to
    // keep this file under the 600-line cap.
}
