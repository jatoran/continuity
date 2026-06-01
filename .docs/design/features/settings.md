# Settings

TOML schema at `settings.toml`, parsed and validated at load time. A single watcher fans `ConfigEvent::Changed` out to every live window for hot reload. The full type definition lives in `crates/config/src/settings.rs::Settings`; live-toggle handlers in `Window::apply_settings` keep on-screen state in sync without restart.

## What it is
- TOML-loaded settings at the runtime config path: `%APPDATA%\continuity\settings.toml` for normal launches, or `<exe>\data\settings.toml` when `--portable` is passed or a `data\` directory sits beside the executable. Hot-reloaded by a single watcher; fan-out to live windows. Validated at parse time. The full schema lives in `crates/config/src/settings.rs::Settings`.

## Key concepts
- **`Settings`** — `serde(default)` top-level struct: `[editor]`, `[markdown]`, `[ui]`, `[window]`, `[workers]`, `[persistence]`, `[backup]`. Defaults match spec §§9–11 plus worker-health defaults.
- **`SettingsWatcher`** — owns the `notify` watcher; sends `ConfigEvent` to subscribers on debounced file change events (`DEFAULT_DEBOUNCE = 250 ms`).
- **`apply_settings(&Settings)`** — the Window-side fan-out method; rebuilds `view_options`, theme, fonts, keymap, motion policy, view-state defaults from the snapshot.
- **`open_settings`** (Phase A3) — opens `settings.toml` as a buffer inside continuity; subsequent `Ctrl+S` writes through the file-I/O thread; watcher reloads. Path resolution routes through `ensure_settings_file`, which seeds the file on first run *and* applies the one-shot legacy-cheatsheet strip on existing files.
- **`ensure_settings_file`** — seeds a brand-new file with `SETTINGS_TEMPLATE` (a 4-line VSCode-style header pointing to `.docs/generated/SETTINGS.md`). On an existing file, runs `strip_legacy_cheatsheet`: detects the pre-VSCode-style chatty template by its marker line (`# This file is hot-reloaded:`) and trims the cheatsheet block — *and* the contiguous comment block above it — through EOF. Live overrides above the marker are preserved byte-for-byte. Migration is idempotent (a file without the marker is left untouched).
- **Writeback helpers** (contract (C) in `../defaults.md`) — `Window::persist_boolean_setting` / `persist_string_setting` / `persist_float_setting` (+ `_or_log` soft-failure companions) share a private `persist_scalar_setting` core that does comment-preserving `toml_edit` rewrite + atomic temp-file write + bumps `SettingsProjections::writeback_in_flight`. The next inbound watcher event is recognized as our own echo and `apply_settings` is skipped.

## Settings shape (excerpt)

```toml
[editor]
font_family_prose   = "Segoe UI Variable"   # Phase E9
font_family_mono    = "Cascadia Mono"
font_size           = 14.0
line_height         = 1.35                  # row stride = round(scaled_font_size × this)
word_wrap           = true
ruler_columns       = []
caret_style         = "bar"                 # "bar" | "block" | "underline"
caret_blink_ms      = 530
caret_width_px      = 2                     # Phase B4
caret_blink_on_typing_pause = true          # Phase B5
caret_typing_pause_ms = 400
caret_color         = ""                    # "" = fall through to theme
caret_secondary_color = ""
caret_tween_enabled = true                  # Phase B7
caret_tween_threshold_rows = 5
caret_tween_duration_ms = 160
auto_pair_paren     = false                 # Phase B8 / J7
auto_pair_bracket   = false
auto_pair_brace     = false
auto_pair_dquote    = false
auto_pair_squote    = false
auto_pair_backtick  = false
auto_pair_asterisk  = false
auto_pair_underscore = false
autolink_bare_urls  = true                  # Phase B12
trim_trailing_whitespace_on_save = true     # Phase B14
show_soft_wrap_indicator = true             # Phase B17
soft_wrap_indicator_glyph = "↪"
autocorrect_enabled = false                 # Phase B18
ligatures           = false
smooth_scroll       = true
mouse_wheel_scroll_speed = 2.0
zoom_step_pct       = 10

[markdown]
reveal_mode    = "block"                    # "block" | "line"
heading_scale  = [2.0, 1.6, 1.35, 1.2, 1.1, 1.05]

[ui]
show_status_bar  = true
show_line_numbers = true                    # Phase A4
show_minimap     = false
tab_close_button = "hover"                  # "hover" | "always" | "never"
theme_mode       = "system"                 # "dark" | "light" | "system"
theme_dark       = "deep_minimal"           # Phase E5 default
theme_light      = "paper"
reduced_motion   = false                    # α.0 shared motion contract

[window]
restore_to_virtual_desktops = true

[workers]
decoration_watchdog_ms = 2000

[persistence]
mode                       = "normal"       # "safe" | "normal"
snapshot_every_edits       = 500
snapshot_every_kib         = 256
snapshot_every_seconds     = 60

[backup]
cadence_minutes = 15
daily_retention = 30
```

## Operations

### Load
1. `Settings::default()` produces the spec defaults.
2. `Settings::from_toml_validated(&str) -> Result<Settings, Error>` parses + runs `validate()`.
3. `validate()` checks ranges (font_size 6..=72, caret_width 1..=16, caret_tween_duration_ms 0..=2000, mouse_wheel_scroll_speed 0.25..=8.0, workers.decoration_watchdog_ms 100..=600000, etc.), enum parses (`CaretStyle::parse`, `RevealMode::parse`, `ThemeMode::parse`, `TabCloseButton::parse`, `PersistenceMode::parse`), and bespoke validators (`validate_caret_color` for hex / theme-key / empty).
4. Settings load failure → keep the previous settings; UI shows a banner.

### Hot reload
1. `SettingsWatcher` debounces `notify` events (default 250 ms).
2. On change, parse the new file. If validation passes, broadcast `ConfigEvent::Settings(Arc<Settings>)` to subscribers.
3. UI windows poll `control_rx` on a `WM_TIMER` (CONFIG_POLL_TIMER_ID), drain the event, call `apply_settings(&new)`.

### Per-buffer apply
- `apply_settings` rebuilds `view_options` (every `editor.*` toggle, `editor.mouse_wheel_scroll_speed`), font family / size scaling, theme name resolution, auto-pair config, motion policy (`ui.reduced_motion`), view-state defaults (word_wrap, ruler_columns). `editor.line_height` lands on `SettingsProjections.line_height_multiplier`, consumed by `Window::effective_line_height()` (`= round(scaled_font_size × line_height_multiplier)`) — the canonical row stride for paint and all vertical geometry (see `rendering.md` § Effective line height).
- The watcher itself lives in the app crate (`app::registry`); each window subscribes during startup so single-watcher / fan-out semantics hold.

### `settings.open` (Phase A3 + E1)
- Opens `settings.toml` as a buffer in continuity (no `ShellExecuteW` to Notepad anymore).
- Path resolution calls `ensure_settings_file` first; first-run users get the minimal seed, legacy installs get the cheatsheet stripped silently.
- Subsequent `Ctrl+S` writes via the file-I/O thread; the watcher catches the change and reloads.
- `ShellExecuteW` is the fallback when the file-I/O thread isn't available (test stubs, headless).
- Phase E1 follow-up: validation errors surface as non-blocking banners; a Win32 modal dialog (spec §14) may also be added later.

### Seed + legacy cheatsheet migration
- Brand-new `settings.toml` carries `SETTINGS_TEMPLATE` only — a 4-line header pointing at `.docs/generated/SETTINGS.md` for the full key list. The user file is overrides-only (VSCode / Sublime model); defaults live in the binary.
- Earlier installs were seeded with a chatty template that pasted every default into the file as commented `# key = default` lines. Once writebacks started appending live overrides above that block (toml_edit can only update / append uncommented entries; it never uncomments existing comments), users saw apparent duplication (`font_family_prose = "Consolas"` live + `# font_family_prose = "Segoe UI Variable"` dead).
- `strip_legacy_cheatsheet` detects the legacy marker (a single, full-line-anchored constant) and trims tailward through EOF. It does *not* parse TOML — it only removes comment / blank lines above the marker until a non-comment line is reached, so live overrides are never touched.
- One-shot: idempotent. A migrated file has no marker; the next `ensure_settings_file` call is a no-op.

### Writeback contract (C)
- See `../defaults.md` "Hot-reload contract" for the full classification: (C) bidirectional sync covers boolean toggles *and* scalar commit commands (font family, font size). (A) covers exploratory scalar commands (theme cycle). (B) covers launch-only with explicit justification.
- Commit sites use `Window::persist_string_or_log` / `persist_float_or_log` for the font picker confirm path (`confirm_font_picker` in `window_overlay_confirm.rs`) and the legacy `ChooseFontW` fallback. The picker's preview path (`set_font_family`) does *not* persist — only the explicit commit on Enter does, so arrow-step previews stay ephemeral.
- The watcher echo is consumed once per writeback via `Window::consume_writeback_echo`, which decrements `writeback_in_flight` and short-circuits `apply_settings`. Concurrent watcher events outside the echo still reproject normally.
- **Font commits compose two flows.** `confirm_font_picker`, the `ChooseFontW` fallback, `set_font_size_impl`, and both `apply_settings` font branches run *both* the (C) writeback (`persist_*_or_log`) *and* the deferred font swap (`Window::request_font_change`) — persistence is unconditional and immediate (the user's intent is durable even if they close the window before the visible swap lands), while the *visible* font change is deferred until the projection worker has a display map for the new font. See `rendering.md` § Operations → "Deferred font swap" for the swap mechanics. The two flows are independent: persistence runs whether or not the deferred swap eventually fires.

## API surface
- `crates/config/src/settings.rs::Settings` (`serde(default)`, typed) + `Settings::from_toml_validated`.
- `crates/config/src/workers.rs::WorkerConfig` for worker-health knobs.
- `crates/config/src/validate.rs::validate` (and the helpers it routes through).
- `crates/config/src/watcher.rs::{SettingsWatcher, ConfigEvent, WatchPaths, DEFAULT_DEBOUNCE}`.
- `crates/config/src/mode.rs::{CaretStyle, PersistenceMode, RevealMode, TabCloseButton, ThemeMode}` typed enum parsers.
- Window-side fan-out: `crates/ui/src/window_settings_reload.rs::apply_settings`.
- Seed + migration: `crates/ui/src/window_settings_seed.rs::{SETTINGS_TEMPLATE, ensure_settings_file, strip_legacy_cheatsheet}`.
- Writeback: `crates/ui/src/window_settings_persist.rs::{persist_boolean_setting, persist_string_setting, persist_float_setting}` plus `_or_log` companions and the private `persist_scalar_setting` core.

## Configuration
N/A — this *is* the configuration system.

## Key files
- schema: `crates/config/src/settings.rs`
- validator: `crates/config/src/validate.rs`
- watcher: `crates/config/src/watcher.rs`
- typed enums: `crates/config/src/mode.rs`
- error: `crates/config/src/error.rs`
- autocorrect schema (related, lives in same crate): `crates/config/src/autocorrect.rs`
- app-level wiring: `crates/app/src/registry.rs`
- window-side apply: `crates/ui/src/window_settings_reload.rs`
- seed + legacy migration: `crates/ui/src/window_settings_seed.rs`
- writeback helpers: `crates/ui/src/window_settings_persist.rs`
- writeback echo counter: `crates/ui/src/window_settings_projections.rs`

## Relates to
- [Autocorrect](autocorrect.md) — separate TOML file, same hot-reload pipeline.
- [Theme](theme.md) — `[ui]` block carries theme names + mode.
- [Caret presentation](caret.md) — `editor.caret_*` settings drive renderer + blink + tween.
- [Motion](../motion.md) — `[ui].reduced_motion` is the zero-frame motion toggle.
- [Persistence](persistence.md) — `[persistence]` + `[backup]` blocks.
- [Rendering](rendering.md) — font-family / font-size commits compose the (C) writeback here with the deferred font-swap pipeline on the render side.
