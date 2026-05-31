# Autocorrect

User-editable literal-or-regex replacement rules at `autocorrect.toml`. Off by default; opt-in via `[editor].autocorrect_enabled`. Hot-reloaded by the settings watcher.

## What it is
- User-editable literal-or-regex replacement rules at `%APPDATA%\continuity\autocorrect.toml`. Fires on space / punctuation following a matched pattern. Default off (Phase B18). Hot-reloaded via the existing settings watcher.

## Key concepts
- **`AutocorrectRule { pattern, replacement, word_boundary }`** — literal text → literal replacement; `word_boundary` defaults to `true` (only match at start-of-line or after a non-word char).
- **`AutocorrectRuleset { rules }`** — top-level TOML; `[[rule]]` blocks.
- **Trigger** — any whitespace or `.,;:!?)]}` character. `is_autocorrect_trigger(ch)` is the predicate.
- **`AutocorrectMatch { start, end, replacement }`** — what the firing rule plans to replace.

## TOML shape

```toml
[[rule]]
pattern     = "teh"
replacement = "the"
word_boundary = true    # default; can omit

[[rule]]
pattern     = "->"
replacement = "→"
word_boundary = false
```

## Operations

### Decision oracle
```rs
fn first_match(text: &str, caret_byte: usize, trigger: char, rules: &[AutocorrectRule]) -> Option<AutocorrectMatch>;
```

Walks `rules` in declaration order. First rule whose pattern ends *exactly* at `caret_byte` (preceded by appropriate word boundary if `word_boundary`) wins. The function is pure — no buffer access, no allocation beyond the returned `Option`.

### Insert hook (follow-up)
The trigger hook on `editor.insert_char` will be:

```rs
fn on_char(&mut self, ch: char) -> bool {
    if !ch_is_printable(ch) { return false; }
    self.note_input_now();
    if self.overlays.is_active() { return self.overlay_on_char(ch); }
    // Phase B18 — propose autocorrect before inserting.
    if self.view_options.autocorrect_enabled && is_autocorrect_trigger(ch) {
        let snap = self.current_snapshot()?;
        let rope_text = snap.rope_snapshot().rope().to_string();
        let caret_byte = caret_byte_offset(snap.selections);
        if let Some(m) = autocorrect_first_match(&rope_text, caret_byte, ch, &self.autocorrect_rules) {
            // Replace [m.start..m.end] with m.replacement, then insert `ch`.
            // One undo group.
            …
        }
    }
    self.dispatch_command(EDITOR_INSERT_CHAR.as_str(), &Value::String(ch.to_string()))
}
```

The hook itself is not wired yet — Phase B18 shipped the schema + decision oracle. The wire-up is a single `Context::on_autocorrect_trigger` call site.

### Hot reload
- `SettingsWatcher::WatchPaths` accepts additional paths beyond `settings.toml`. The app wiring adds `autocorrect.toml` so file changes broadcast through the same `ConfigEvent` fan-out.
- Window-side: `apply_settings` updates `Window::autocorrect_rules: Vec<AutocorrectRule>` from the latest `AutocorrectRuleset`.

## API surface
- `crates/config/src/autocorrect.rs`:
  - `AutocorrectRule`, `AutocorrectRuleset`, `AutocorrectMatch`.
  - `is_autocorrect_trigger(ch: char) -> bool`.
  - `first_match(text, caret_byte, trigger, &rules) -> Option<AutocorrectMatch>`.
  - `AutocorrectRuleset::from_toml(src) -> Result<Self, toml::de::Error>`.

## Configuration
- `editor.autocorrect_enabled` (default `false` — opt-in).
- File path: `%APPDATA%\continuity\autocorrect.toml` (or `${PORTABLE_ROOT}\autocorrect.toml` in portable mode).

## Key files
- schema + oracle: `crates/config/src/autocorrect.rs`
- crate re-exports: `crates/config/src/lib.rs`
- (future) insert-path wiring: `crates/ui/src/window_commanding.rs::Context::on_char`

## Relates to
- [Settings](settings.md) — same hot-reload pipeline.
- [Command system](command-system.md) — the wire-up sits at the `editor.insert_char` handler, one `Context` method away.
- [Selections + edits](selection-edits.md) — when a rule fires, the trigger insert + replacement land as one `SelectionEdit::InsertText` covering the replaced run + trigger char (one undo group).
