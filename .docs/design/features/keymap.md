# Keymap

TOML-driven chord → command-id bindings. The bundled default keymap (`crates/keymap/assets/default.toml`) is layered under a user keymap at `keymap.toml`; user bindings win on collision. Multi-key sequences (e.g. `Ctrl+K, Ctrl+S`) ride on a per-window pending-chord accumulator.

## What it is
- TOML-driven chord → command-id bindings. Layered: defaults (bundled) + user overrides (`%APPDATA%\continuity\keymap.toml`). Conflict checker reports collisions. Multi-key sequences (`Ctrl+K Ctrl+S`) are supported via a pending-chord accumulator on the window.

## Key concepts
- **`KeyChord { vk: u16, modifiers: Modifiers }`** — one keypress; produced from `WM_KEYDOWN` (`vk`) + `GetKeyState` (`Modifiers`).
- **`Binding { keys: Vec<KeyChord>, command: String, args: Option<Value>, when: Option<ContextPredicate> }`** — one entry in TOML. `keys` length >1 ⇒ multi-chord sequence.
- **`Keymap`** — flat list of bindings; `lookup(chord, ctx)` resolves the first binding whose predicate satisfies and chord matches; `match_sequence(seq, ctx)` returns `Match | Prefix | None` for the accumulator.
- **Pending chord accumulator** — `Window::pending_chord_sequence` collects chords mid-sequence; cleared on dispatch or on first non-matching chord.
- **Conflict checker** — `Keymap::detect_conflicts` reports same-chord different-command (predicate-permitting) so the UI can show a banner.

## Data model

```rs
struct Modifiers { ctrl: bool, shift: bool, alt: bool, win: bool }
struct KeyChord  { vk: u16, modifiers: Modifiers }

struct Binding {
    keys: Vec<KeyChord>,
    command: String,
    args: Option<Value>,
    when: Option<ContextPredicate>,
}

enum SequenceMatch<'a> { Match(&'a Binding), Prefix, None }
```

TOML format (default.toml):
```toml
[[binding]]
keys = ["ctrl+s"]
command = "file.save"

[[binding]]
keys = ["ctrl+k", "ctrl+r"]
command = "markdown.refresh_toc"

[[binding]]
keys = ["tab"]
command = "editor.indent"
# `when` defaults to "editor.focused"
```

Chord syntax (parser in `keymap::parser`):
- Modifiers: `ctrl`, `shift`, `alt`, `meta` (Win key). Separated by `+`. Case-insensitive.
- Keys: `a`..`z`, `0`..`9`, `f1`..`f24`, named (`escape`, `tab`, `enter`, `space`, `home`, `end`, `pgup`, `pgdn`, `up`, `down`, `left`, `right`, `backspace`, `delete`, `insert`), and printable symbols (`-`, `=`, `[`, `]`, `\\`, `;`, `'`, `,`, `.`, `/`, `` ` ``).
- Multi-chord sequences: `["ctrl+k", "ctrl+s"]` (TOML array).

## Operations

### Overlay-scoped defaults
Find-bar bindings are predicate-gated on `find_bar.visible`, so they are inert in normal editing unless another binding owns the same chord in a narrower context.

| Chord | Command | Find-bar behavior |
|---|---|---|
| `Alt+C` | `editor.find_toggle_case` | Toggle case-sensitive (`Aa`). |
| `Alt+W` | `editor.find_toggle_word` | Toggle whole-word (`|w|`). |
| `Alt+R` | `editor.find_toggle_regex` | Toggle regex (`.*`). |
| `Alt+P` | `editor.find_toggle_preserve_case` | Toggle preserve-case replace (`AB`). |
| `Alt+S` | `editor.find_toggle_scope` | Toggle whole-buffer vs selection scope (`All` / `Sel`). |
| `Alt+Enter` | `editor.find_matches_to_cursors` | Convert current matches into cursors. |
| `Ctrl+Enter` | `editor.find_replace_one` | Replace current match when replace mode is visible. |
| `Ctrl+Shift+Enter` | `editor.find_replace_all` | Replace all matches. |
| `Ctrl+Alt+Enter` | `editor.find_replace_all` | Replace all matches; mirrored in the find-bar tooltip. |

### Dispatch loop (`Window::on_keydown`)
1. Build a `KeyChord` from the live modifier set.
2. If an overlay is active → `overlay_on_keydown` consumes the key; fall through to keymap only if the overlay declines.
3. **Phase B3** Esc intercept: if no overlay and `vk == VK_ESCAPE`, run `dismiss_priority_chain` first (banner → view-overlay revert → …). If it consumes, stop.
4. Push the chord onto `pending_chord_sequence`.
5. `match_sequence(seq, self)`:
   - `Match(binding)` → dispatch the binding's command; clear pending.
   - `Prefix` → keep pending; wait for next chord.
   - `None` and `seq.len() > 1` → drop the first chord, try the latest chord standalone (so a stray `Ctrl+K` followed by `Ctrl+S` still saves).
   - `None` and `seq.len() == 1` → no-op.

### Conflict checker (J4)
`Keymap::detect_conflicts` returns `Vec<Conflict>` of same-chord same-context different-commands. `Window::refresh_keymap_conflicts` runs on construction and on every keymap reload; results live in `Window::keymap_conflicts` and can be surfaced by `keymap.show_conflicts`.

### Layered loading
1. Load `crates/keymap/assets/default.toml` (bundled, `include_str!`).
2. If `%APPDATA%\continuity\keymap.toml` exists, parse it and overlay — later entries take precedence over earlier.
3. Settings watcher reloads on save; UI calls `keymap.reload` and refreshes conflicts.

## API surface
- `Keymap::from_toml(default_toml, user_toml: Option<&str>) -> Result<Keymap, Error>`
- `Keymap::lookup(&self, chord: &KeyChord, ctx: &dyn Context) -> Option<&Binding>`
- `Keymap::match_sequence(&self, seq: &[KeyChord], ctx: &dyn Context) -> SequenceMatch<'_>`
- `Keymap::detect_conflicts(&self) -> Vec<Conflict>`
- `commands` re-exported `KEYMAP_RELOAD`, `KEYMAP_SHOW_CONFLICTS` (in `command::editor`).

## Configuration
- `%APPDATA%\continuity\keymap.toml` — user overrides.
- No `[keymap]` section in `settings.toml`; binding data lives in its own file so the conflict checker can run standalone.

## Key files
- types: `crates/keymap/src/lib.rs`
- chord parser: `crates/input/src/chord.rs` (Win32 vk translation)
- conflict checker: `crates/keymap/src/conflict.rs`
- default bindings: `crates/keymap/assets/default.toml`
- input crate (Win32 raw key parsing): `crates/input/src/lib.rs`, `crates/input/src/chord.rs`

## Relates to
- [Command system](command-system.md) — bindings reference command ids; dispatch goes through `Registry`.
- [Interfaces](../interfaces.md) — predicate grammar.
- [Overlays](overlays.md) — overlays preempt keymap dispatch.
- [Settings](settings.md) — hot reload picks up `keymap.toml` saves.
