# Spell check

Per-buffer opt-in spell check via Windows `ISpellChecker` (COM). Squiggle decoration painted under the body text; right-click surfaces suggestions. Off by default (spec §J3); toggle via `spell.toggle`.

## What it is
- Per-buffer opt-in spell-check via Windows `ISpellChecker` (COM). Squiggle decoration painted under the body text; right-click suggestions. Disabled by default (spec §J3).

## Key concepts
- **`SpellChecker`** — wraps `ISpellCheckerFactory` + `ISpellChecker` COM interfaces. One per UI thread; locale follows the user's Windows display language.
- **`SpellState`** — per-window state machine: `{ enabled: bool, errors: AHashMap<BufferId, Vec<SpellError>> }`. Live errors recomputed on revision change (when enabled).
- **`SpellError { start_byte, end_byte, word }`** — single misspelling occurrence in source coordinates.
- **`SpellSquiggleSpan`** — renderer-side `(line, byte_in_line_start, byte_in_line_end)` flattened from `SpellError`.

## Operations

### Toggle
- `editor.spell_check` (default `false`). Per-buffer override stored in `Buffer::file_association` metadata (TBD; currently buffer-scoped state lives on `Window::spell_state`).
- `view.toggle_spell_check` flips the flag.

### Compute (on enable / on revision change)
1. Window receives `EditEvent::EditApplied { id, revision }`.
2. If spell is enabled for `id` and the buffer's `Decorations` distinguish prose runs, schedule a re-check.
3. Walk inline prose spans (skip `Code`, `FenceTick`, `Link.url_range`, etc.).
4. For each word, `ISpellChecker::Check(word) -> ISpellingError`. Misspellings produce `SpellError` rows.
5. `SpellState::errors[buffer_id] = …`; invalidate window.

### Paint
- `Window::build_spell_squiggle_spans(rope)` translates `SpellError` rows into `SpellSquiggleSpan` rows in source coordinates.
- `crates/render/src/spell.rs::paint_spell_spans` converts each row's `byte_in_line` range into display utf16 coords via `FrameDisplay::source_byte_in_line_to_display_utf16`, then draws a squiggle under the run.

### Suggestions (right-click)
- `WM_CONTEXTMENU` over a misspelled word opens a popup via `crates/ui/src/window_context_menu.rs`.
- Menu items: top 5 suggestions from `ISpellChecker::Suggest(word)`, then "Add to dictionary", "Ignore once".

## Configuration
- `editor.spell_check` (default `false`).
- `spell.suggestion_limit` (default `5`).
- `spell.ignore_words` — user list of words to skip. Hot-reloaded.

## Key files
- COM wrapper: `crates/ui/src/window_spell.rs`
- spell-state on Window: `crates/ui/src/window_spell.rs::SpellState`
- IME / WM_CONTEXTMENU plumbing: `crates/ui/src/window_context_menu.rs`
- squiggle paint: `crates/render/src/spell.rs`
- squiggle data shape: `crates/render/src/params.rs::SpellSquiggleSpan`

## Relates to
- [Decoration](decoration.md) — only prose runs are spell-checked; code / fence / url ranges are excluded.
- [Rendering](rendering.md) — squiggles paint under text via `crate::spell::paint_spell_spans`.
- [Settings](settings.md) — toggle + suggestion limit.
