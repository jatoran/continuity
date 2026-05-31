# Theme

TOML-loaded color sets keyed by stable name. Seventeen bundled themes ship with the binary (`deep_minimal`, `paper`, `solarized_dark`, `solarized_darker`, `solarized_light`, `monokai`, `rose_pine`, `catppuccin_mocha`, `catppuccin_macchiato`, `catppuccin_frappe`, `catppuccin_latte`, `tokyo_night`, `nord`, `one_dark`, `gruvbox_dark`, `gruvbox_light`, `dracula`); user customs live under the runtime themes directory (`%APPDATA%\continuity\themes\` normally, `<exe>\data\themes\` in portable mode) and are managed entirely through the δ.5 workflow commands. Bundled themes are read-only — edits surface a clone-first banner.

## What it is
- TOML-loaded color sets keyed by stable string names. Seventeen bundled themes (`deep_minimal`, `paper`, `solarized_dark`, `solarized_darker`, `solarized_light`, `monokai`, `rose_pine`, `catppuccin_mocha`, `catppuccin_macchiato`, `catppuccin_frappe`, `catppuccin_latte`, `tokyo_night`, `nord`, `one_dark`, `gruvbox_dark`, `gruvbox_light`, `dracula`) plus an embedded neutral fallback. User-installed customs live under the runtime themes directory and are managed entirely from inside continuity via the δ.5 workflow commands — the filesystem is an implementation detail the user shouldn't have to touch.
- Bundled themes are baked into the binary (`include_str!`) and **read-only**. Edits or deletes against a bundled theme surface a "clone first" banner. Customs are freely renamed, edited, and soft-deleted; the underlying TOMLs are still hot-reloaded by the settings watcher.

## Key concepts
- **`Theme { name, colors: AHashMap<String, Color> }`** — flat key → color map.
- **`Color`** — `Rgba { r, g, b, a }`; parsed from `#rrggbb` or `#rrggbbaa` hex.
- **`REQUIRED_KEYS`** — flat array of dot-separated keys every theme must declare. `Theme::validate_required` runs at parse time so consumers can call typed accessors without `Option` plumbing.
- **`ThemeMode`** — `Dark` | `Light` | `System` (default). `System` follows Windows.
- **`ActiveTheme`** — Window-side state machine: mode + system-dark flag + currently resolved `Arc<Theme>`. `cycle_mode` walks `Dark → Light → System → Dark`.

## Required key namespaces
- `window.*` — full-window chrome (window background, foreground).
- `panel.*` — tab strip backgrounds, active/inactive tab fg/bg.
- `pane.border` / `pane.border_active` — focused-pane accent.
- `editor.*` — body fg/bg, cursor, selection, line highlight, line numbers, indent guides, search match colors, find-bar bg.
- `editor.caret_jump_glow` (Phase B6) — RGBA tint for the destination row after a long caret jump.
- `editor.pair_rainbow.0..5` (Phase B8) — 6-level nested bracket palette.
- `editor.soft_wrap_indicator` (Phase B17) — margin glyph color.
- `markdown.*` — heading 1–6, bold, italic, strikethrough, code, link, footnote, checkbox, hr, table border.
- `status.*` — reserved for the C-phase status bar segments.
- `overlay.*` — palette, find bar, banner backgrounds.

## Typed accessors

Themes expose `Theme::editor_background()`, `Theme::editor_cursor_primary()`, `Theme::editor_caret_jump_glow()`, `Theme::editor_pair_rainbow(level)`, `Theme::editor_soft_wrap_indicator()`, `Theme::markdown_heading(level)`, etc. Each typed accessor calls `self.required(KEY).expect("invariant: REQUIRED_KEYS")` — a panic here means the key was added to `REQUIRED_KEYS` but a bundled theme TOML wasn't updated, which the asset test catches.

## Operations

### Load
1. `Theme::from_toml(src)` parses + validates required keys; `Theme::validate_required` returns `Err(Error::MissingKey)` listing any missing keys.
2. `bundled_set()` loads the two bundled themes via `include_str!`.
3. `installed_themes(dir)` scans the runtime `themes\*.toml` directory and parses each; failures log + skip (don't crash).
4. `resolve_active(theme_name, mode, system_dark, installed, bundled)` returns the best match: name → installed → bundled → neutral fallback.

### Live reload
- Settings watcher (`config::SettingsWatcher`) re-walks `themes/` on file change events.
- `Window::apply_settings` calls `ActiveTheme::reload`; renderer brushes are re-derived from the new `Arc<Theme>`.
- Reload also applies `[ui].reduced_motion` to the Window motion policy. Any region motion caused by the same reload/reflow batch must request a `StaggerScheduler` slot from the shared motion contract.

### Preview vs commit
- **Preview** (`Window::apply_theme_entry`) — picker hover loop only.
  Mutates `active_theme.set` in memory; never writes settings.toml,
  never broadcasts to siblings. Esc reverts via `tp.revert_set()`.
- **Commit** (`Window::commit_theme_entry`) — picker `Enter`. Writes the
  active slot's name to `settings.toml` via
  `write_settings_theme_binding` (atomic write) and synchronously
  refreshes the shared `LiveReload.initial` cell *before* the watcher
  echo arrives. Does NOT mutate `active_theme` directly — the in-memory
  swap happens exactly once, inside `Window::apply_settings_theme_bindings`,
  driven by `ConfigEvent::Settings` from the watcher. Same code path
  fires for the source window and every sibling.
- **Active-slot resolution.** `active_theme_slot(active)` maps
  `(mode, system_dark)` → `ThemeSlot::Dark` or `ThemeSlot::Light`. Only
  the active slot is rewritten on commit; the non-active slot is
  preserved.
- **Mistyped theme names.** `resolve_theme_by_name` returns `None` and
  the slot is left alone rather than blanked. The prior valid theme
  stays active until the user fixes the typo.

### Newly-spawned windows see the latest theme
The registry's `LiveReload.initial` field is wrapped in
`Arc<Mutex<Settings>>` (`continuity_ui::LiveReload::current_settings` /
`replace_settings`). Two paths feed it: (a) the watcher echo in
`fan_out_config_event` calls `replace_settings` before broadcasting
`WindowControl::ConfigChanged`, so the cell update precedes the
fanout; (b) `commit_theme_entry` re-reads `settings.toml` and calls
`replace_settings` synchronously after the disk write returns, closing
the race window between a runtime commit and the next window spawn.
Any window opened after a commit reads the freshly-committed theme
from the cell on `maybe_apply_initial_settings`.

### Per-Window `ActiveTheme`
```rs
struct ActiveTheme {
    mode: ThemeMode,
    system_dark: bool,
    theme: Arc<Theme>,
}
impl ActiveTheme {
    fn editor_colors(&self) -> EditorColors;
    fn markdown_colors(&self) -> MarkdownColors;
    fn cycle_mode(&mut self);
    fn set_system_dark(&mut self, dark: bool);
    fn reload(&mut self, …);
}
```

`EditorColors` + `MarkdownColors` are the renderer-facing flat structs; `apply_settings` rebuilds them on every reload tick.

## API surface
- Public from `crates/theme/src/lib.rs`: `Theme`, `Color`, `ThemeMode`, `REQUIRED_KEYS`, `bundled_set`, `installed_themes`, `resolve_active`, `Error`.
- Window-side: `crates/ui/src/window_theme.rs::ActiveTheme` + `EditorColors` + `MarkdownColors`.

## Configuration
- `[ui] theme_dark` / `theme_light` / `theme_mode` (`"dark" | "light" | "system"`) in `settings.toml`. The δ.5 install commands (`theme.clone`, `theme.duplicate`, `theme.create_blank`) rewrite these keys in place, preserving comments and unrelated keys.
- User-installed themes drop into the runtime themes directory (`%APPDATA%\continuity\themes\<name>.toml`, or `<exe>\data\themes\<name>.toml` in portable mode). Soft-deleted customs live under `themes\.trash\<name>-<unix-ms>.toml` for recovery.
- Theme picker (Phase E4) opens the palette in theme mode; δ.5 adds row-level inline actions: **Ctrl+E** edits, **Ctrl+D** duplicates, **Ctrl+Backspace** soft-deletes a custom row. Bundled rows expose only Enter + Ctrl+D and surface a banner when the user attempts edit or delete.

## δ.5 — workflow commands

Every command is `palette_safe` and gated on `editor.focused`. Inputs come as JSON args so the palette path and chord/keymap path share one handler. Names are sanitized via [`continuity_theme::check_theme_name`]: ASCII alphanumeric + `_-`, max 64 chars, no leading dot, reserved names checked programmatically against `BUNDLED_NAMES`. Rejection surfaces a banner; the command no-ops.

| Command | Args | What it does |
|---|---|---|
| `theme.clone` | `{name?: string}` | Clone the active theme into a new editable custom. Auto-names `<active>-copy` when no name is supplied. Activates the new theme and updates the matching `[ui]` slot in `settings.toml`. |
| `theme.edit` | `{name?: string}` | Open the named theme's TOML as a new tab. Defaults to the active theme. Bundled themes surface a clone-first banner. |
| `theme.duplicate` | `{source?: string, name?: string}` | Clone any installed theme (bundled or custom). Same auto-naming and activation as `theme.clone`. |
| `theme.rename` | `{old?: string, new: string}` | Rename a custom theme on disk. Updates `[ui] theme_dark` / `theme_light` when the renamed theme is currently bound. Rejects bundled targets and name collisions. |
| `theme.delete` | `{target?: string}` | Soft-delete a custom theme by moving it under `themes/.trash/`. Falls back to `deep_minimal` and rewrites the binding when the deleted theme was active. |
| `theme.reveal_folder` | (none) | Open the runtime themes directory in Explorer. Escape hatch for copy-between-machines or sharing. |
| `theme.create_blank` | `{name?: string}` | Write a minimal valid theme (every `REQUIRED_KEYS` entry populated from `neutral_fallback()`) and open it for editing. Default auto-name is `custom`. |

### Live validation when editing a theme TOML
When the user saves an edited TOML inside continuity, the `SettingsWatcher` re-fires `ConfigEvent::Theme`. The Window-side handler runs `Theme::load` (parse + required-key validation + per-color hex parse). On success the new theme swaps into the active slot. On failure the previous theme stays active and a banner surfaces `theme `<name>`: <reason>`. The TOML stays untouched on disk — the user fixes, saves again, the watcher retries.

### Atomic writes + crash safety
Every theme install writes to `<path>.tmp` and `std::fs::rename`s into place, so a crash mid-write never leaves a partially-corrupt TOML. The settings.toml rewrites use the same dance.

## Key files
- core: `crates/theme/src/theme.rs`
- color parser: `crates/theme/src/color.rs`
- required keys: `crates/theme/src/keys.rs`
- bundled assets + neutral fallback: `crates/theme/src/assets.rs`, `crates/theme/assets/{deep_minimal,paper,solarized_*}.toml`
- theme mode: `crates/theme/src/mode.rs`
- TOML serializer (`Theme::to_toml`): `crates/theme/src/serialize.rs`
- name sanitization (`check_theme_name`, `is_reserved_name`): `crates/theme/src/sanitize.rs`
- Window-side state: `crates/ui/src/window_theme.rs`
- reduced-motion projection: `crates/ui/src/window_settings_reload.rs`, `crates/ui/src/window_motion.rs`
- δ.5 command surface: `crates/command/src/theme.rs`
- δ.5 Window `_impl` bodies: `crates/ui/src/window_theme_manage.rs`
- δ.5 settings.toml binding rewriter: `crates/ui/src/window_theme_settings_edit.rs`
- δ.5 atomic write helper: `crates/ui/src/window_theme_atomic_write.rs`
- preview / commit split: `crates/ui/src/window_theme_apply.rs` (`apply_theme_entry` preview, `commit_theme_entry` commit, `apply_settings_theme_bindings` reload-driven swap)
- settings cell (Arc<Mutex<Settings>>): `crates/ui/src/live_reload.rs`

## Relates to
- [Rendering](rendering.md) — `EditorColors` + `MarkdownColors` are renderer inputs.
- [Settings](settings.md) — `[ui]` block carries theme names + mode.
- [Motion](../motion.md) — reduced-motion and same-batch stagger rules apply to theme reload/reflow.
- [Decoration](decoration.md) — heading scales and emphasis styles are theme-driven.
- [Caret presentation](caret.md) — caret colors fall through to theme keys when user overrides are empty.
