# Defaults + policy reference

Cross-cutting decisions that apply across features. This is the *what* — specific defaults the code must reflect. For the *why* behind these choices, see `principles.md`.

When code disagrees with this doc, fix one of them. When this doc disagrees with `principles.md`, principles win (defaults are derived from principles, not the other way around).

This content was originally maintained in `roadmap_v2.md §J`; that location is now a pointer back here. This is the authoritative home.

---

## Motion policy

Functional-only. No delays where they do not aid comprehension.

The canonical timing table, ease-out-cubic curve, 60 ms stagger rule, status-chip transient rule, and reduced-motion behavior live in [motion.md](motion.md). Defaults here defer to that contract.

Reduced motion is controlled by `[ui].reduced_motion = false` by default. When true, every motion duration resolves to 0, active tweens are cleared, and animation timers are not armed.

Mouse-wheel scrolling uses `[editor].mouse_wheel_scroll_speed = 2.0` by default: the base 3-line notch distance is multiplied to 6 lines/notch before smooth inertia or reduced-motion whole-line jumps apply. Valid range: `0.25..=8.0`.

---

## Modal policy

- **Modals are OK for**: the settings dialog (intentional context switch) and genuinely destructive confirmations that can't be made reversible — today the only remaining instance is permanent purge from trash. Close-with-unsaved is **not** a modal case: the buffer is moved to the 30-day persist trash and the user banners "Closed tab moved to trash — restore from Recently Closed (Ctrl+Shift+T)" instead (see `crates/ui/src/window_close_confirm.rs`). The previous `MessageBoxW` was removed in δ.3.
- **Banners for everything else**: file-watcher reload, file-watcher deletion (δ.3), encoding-mismatch notices on open (δ.3), recovery banner (δ.3), persist write failures + thread-stopped (δ.3), validation errors, find-bar invalid-regex (δ.3), find-replace-all completion count (δ.3), etc.

Test before adding a modal: is this interruption *reversible*? If yes, it's a banner. If no, consider whether the operation itself could be made reversible (trash + restore) — only fall to a modal when reversibility is genuinely impossible.

**Banner lifetime (`crates/ui/src/window_file.rs::FileBanner`):**

- **Transient (auto-dismiss)** — confirmation/info banners that announce a completed action ("Saved …", reload-applied, etc.). Auto-dismiss after `TRANSIENT_BANNER_MS = 2500` ms (`FileBanner::transient`); chrome doesn't linger past the action it announces.
- **Sticky (decision-required)** — banners awaiting a user choice (external-change reload/keep/diff, deletion, encoding mismatch, recovery, persist failure). `FileBanner::new` leaves `expires_at_ms = None`; the banner stays until the user acts (button or `Esc`, via `Window::dismiss_priority_chain`).
- **Placement** — the passive file banner paints below the tab ribbon (`TAB_STRIP_HEIGHT_DIP + RIBBON_GAP_DIP`, `Window::file_banner_overlay`) so it never overlaps the tab strip.

---

## Spell-check

- Off by default. Per-buffer toggle via `editor.spell_check`.
- Uses Windows `ISpellChecker`.

---

## Keymap base

- Defaults start Sublime/VSCode-flavored. Heavy user rebinding expected.
- **Keymap-conflict checker** must be built early so rebinding is safe.

---

## Launch + sessions

- **Launch behavior**: restore last session (all windows, panes, tabs, virtual desktops).
- **Single instance per data dir**: a second `continuity.exe` launch does **not** replay the persisted session (which would duplicate every open window). It forwards its command-line file/folder paths to the running instance over `WM_COPYDATA` and exits; a bare launch just activates the running instance's top-most window. Bypass with `--new-instance` (the e2e insert hook bypasses too). Keyed per database path so portable + installed instances coexist. See `architecture.md` § Process model.
- **Restore without focus theft**: at launch only the most-recently-seen restored window takes the foreground; the rest show with `SW_SHOWNOACTIVATE`. A window restored onto a non-active virtual desktop never activates (activating it there would switch the user's desktop). Runtime-spawned windows (new window, tear-off, file open) and `Ctrl+Shift+T` reopen still activate.
- **No empty panes** (D4).
- Heavy multi-window + virtual-desktop user → `request_state_save` wiring (A8) is critical.

---

## Trash + retention

- Trash retention = 30 days (spec §4 default retained).
- Metrics rows excluded from trash flow (I2).

---

## Editing defaults

- **Auto-pair off** (B8). Top user annoyance.
- **Rainbow pair highlighting on** (B8).
- **Trim trailing whitespace on save = on** (B14).
- **Indentation = tabs** (`[editor].indent_type = "tabs"`). `Tab` inserts one tab character; `indent_width` / `tab_width` default 4. Switching `indent_type` at runtime does not retroactively convert existing indentation (`editor.spaces_to_tabs` / `editor.tabs_to_spaces` do that). `Shift+Tab` outdent strips a leading tab or up to one indent-width of leading spaces, so it works regardless of which the line actually uses.
- **Indent folding on always** (H3).
- **Caret-line highlight on by default** (`editor.caret_line_highlight`, `ViewOptions::current_line_highlight = true` in `crates/ui/src/window_view_options.rs`; toggle `view.toggle_current_line_highlight`, default chord `Ctrl+Alt+L`). This is the band painted behind the *caret* line and is distinct from the mouse-hover band (`editor.line_highlight`).
- **Macros dropped** — no `.` repeat-last, no record/replay.
- **Smart paste with indent dropped** (B13 keeps URL/image smart-paste only).

### File save dialog defaults (`crates/ui/src/window_file_dialogs.rs`)

- **Save defaults to the Markdown file type** — `wide_save_filter` lists "Markdown (\*.md, \*.markdown)" first and `nFilterIndex = 1` selects it.
- **Extensionless names get `.md`** — `lpstrDefExt = "md"` appends `.md` to a name typed without an extension; an explicit extension (e.g. `notes.txt`) is respected.
- **Untitled buffers pre-fill the file name from the tab title** — `sanitize_filename_stem` strips the pin-dot/ellipsis decorations, swaps Windows-reserved characters for spaces, caps the stem at 48 chars, and falls back to `"untitled"` when nothing usable remains.

---

## Buffer model defaults

- **Default font = Segoe UI Variable** (E9).
- **Default dark theme = `deep_minimal`** (E5). Theme mode = `system`.
- **Markdown dialect = GFM-compatible + continuity extensions** (F7).
- **Inline images on by default** (F5).
- **Image storage = shared `%APPDATA%\continuity\images\<hash>.<ext>`**.
- **Auto-link bare URLs on by default** (B12).

---

## Status bar baseline

- Bottom of window, user-configurable segments.
- Default segments: `line:col`, char count, word count, non-empty/total line count, selection stats, live numeric sum, encoding + line endings.
- All segments are click-to-act (C2).

---

## Tab strip baseline

Geometry constants and crowding behavior in `crates/render/src/pane_chrome_layout.rs`; close-button suppression in `crates/render/src/pane_chrome.rs`.

- **Preferred tab width** `TAB_PREFERRED_WIDTH_DIP = 200`; one verbose title is capped here so it can't starve neighbours.
- **Crowded strip shrinks small** — when preferred widths don't fit, slots shrink proportionally toward `TAB_SHRINK_MIN_WIDTH_DIP = 88` (`crowded = true`).
- **Overflow scrolls horizontally** — when even the shrink minimum overflows, slots pin at the shrink minimum, the row scrolls, and `TAB_CHEVRON_WIDTH_DIP = 18` is reserved at each edge for the `‹` / `›` chevrons (`overflowing = true`).
- **Close "x" hidden when crowded** — the per-tab close cell is suppressed while the strip is crowded-but-not-overflowing (`suppress_close_cell = crowded && !overflowing`) and on any tab below `TAB_CLOSE_MIN_TAB_WIDTH_DIP`; paint and hit-test mirror each other. Default close-button visibility is `TabCloseButton::Hover`.

---

## Hot-reload contract (settings ↔ runtime parity)

Every user-visible behavior should be reachable from **both** the command palette (runtime toggling) and `settings.toml` (persistent default + hot reload). When the two surfaces meet at runtime, the contract below decides who wins.

**Default rule by setting type:**

- **(C) Bidirectional sync — for boolean toggles *and* committed scalar picks.** The command mutates runtime state *and* persists the new value back to `settings.toml` via `Window::persist_boolean_setting` (booleans), `Window::persist_string_setting` (strings), or `Window::persist_float_setting` (floats/ints). The watcher then sees the writeback as our own echo (via the in-flight counter) and skips re-applying. Result: the file is always the source of truth; the user's runtime change is durable across relaunch. This is the default for every `bool` toggle and for scalar commit commands whose semantics are "the user picked this value, keep it" — font family (`view.pick_font` / `view.set_font_family`) and font size (`view.set_font_size`) are the current scalar examples. Picker overlays that distinguish a *preview* step from the final commit only persist on commit (Enter), not on preview.

- **(A) TOML wins on reload — for exploratory scalar runtime knobs.** Theme names, image cache budget, snapshot policy thresholds, durations, retention counts, and any other non-boolean knob whose runtime command (where one exists) is genuinely exploratory rather than a commit. The user edits these in the TOML deliberately; the hot-reload projection in `Window::apply_settings` overwrites the runtime value unconditionally. Example: `view.cycle_theme` walks through themes for preview/inspection — the next TOML reload still wins because the theme name on disk is the persistent default, not whatever happened to be cycled to. When promoting a scalar command from (A) to (C), add a `persist_*_setting` call at the commit site and update the table below.

- **(B) Runtime wins; TOML applies only at launch — only with explicit justification.** Reserved for the rare case where bidirectional sync would create infinite-loop write storms or where the setting's semantics are genuinely "what state should the window come up in?" rather than "what state should the window be in right now?" Each (B) setting must carry a comment in its `apply_settings` site naming the reason. Today's (B) examples: `[focus].initial_mode`, `[focus].distraction_free_on_launch`, `[window].restore_to_virtual_desktops` — all are *launch-state* settings whose runtime equivalents (`view.cycle_focus`, `view.toggle_distraction_free`) target the live state directly.

**When adding a new setting/command pair, declare which contract it follows.** Default to (C) for `bool` and for scalar *commit* commands (the user picks a value and expects it to stick); (A) for scalar *exploratory* commands (preview/cycle/zoom-style runtime tweaks where the file remains the durable home); (B) only with a justifying comment. The audit at `../development/archive/audit_settings_surface.md` tracks the per-item categorization.

**Implementation reference:** `crates/ui/src/window_settings_persist.rs` provides the writeback helpers (`Window::persist_boolean_setting`, `Window::persist_string_setting`, `Window::persist_float_setting`, plus soft-failure `_or_log` companions, and `Window::toggle_boolean_setting`). All three thin-wrap a shared `persist_scalar_setting` core that handles toml_edit comment-preserving rewrite + atomic temp-file write + writeback-counter bump. `crates/ui/src/window_settings_reload.rs::apply_settings` is the sole projection point for (A) and (C); the watcher fans events out via `WindowControl::ConfigChanged`. The "I-just-wrote-this" suppression counter lives on `Window::settings_projections.writeback_in_flight` (`crates/ui/src/window_settings_projections.rs`) and is consumed by `Window::consume_writeback_echo` when the next watcher event arrives.

---

## See also

- `principles.md` — the rationale for these choices.
- `CLAUDE.md` — the one-screen ethos summary.
- `00_OVERVIEW.md` — global invariants and key trade-offs.
- `../development/spec.md` — long-form source-of-truth spec (when this doc disagrees with the spec, update both deliberately; usually this doc is the more recent decision and the spec needs updating).
