# Tutorial

A synthetic, read-only buffer covering every user-visible feature and hotkey, opened in a tab in the focused pane. Auto-generated from the per-feature design docs and the default keymap — never hand-written. Opens automatically on first launch; re-openable any time via the `help.tutorial` command.

## What it is

The tutorial is a markdown document that lives at `crates/command/assets/tutorial.md`, embedded into the binary via `include_str!`. When the user invokes `help.tutorial` (or the app detects first launch), the UI constructs a `Buffer::synthetic_read_only(TUTORIAL_MD)`, adopts it through the core thread (skipping persist), and opens it as a tab.

Because the rope is real, the user gets every existing markdown affordance for free: display-map projection, decoration (headings, code fences, inline formatting), find-in-buffer, click-on-link, text selection + copy.

Because the buffer is read-only, edits are rejected before they reach the rope — `Buffer::apply` short-circuits with `Error::ReadOnly`, the core dispatch path checks the same, and any UI path that would mutate the active buffer falls through to a banner.

Because the buffer is synthetic, persist never sees it: no `buffers` row, no `buffer_edits`, no `buffer_snapshots`, and no persisted-list entry for search-adjacent surfaces. The synthetic flag is checked in core's `AdoptBuffer` handler before issuing `persist.touch_buffer`.

## Content sources (auto-generation)

The asset is produced deterministically by `cargo xtask gen-tutorial` from two sources that agents already maintain as part of normal feature work:

| Source | Surface |
|---|---|
| `.docs/design/features/*.md` | One `### <Title>` section per feature, taken verbatim as the prose between the H1 and the first H2 of each file. Files without an intro paragraph are skipped — adding a one-paragraph intro to a feature doc surfaces it in the tutorial on the next regen. |
| `crates/keymap/assets/default.toml` | A `## Hotkeys` section as tables grouped by command prefix (`editor.*`, `view.*`, `markdown.*`, …) and sorted within each group. |

Output is byte-stable across regens (sorted entries, fixed header). The CI drift check (`conventions:tutorial-drift`) regenerates the asset in memory and diffs against the checked-in file; any mismatch fails CI with the fix message `cargo xtask gen-tutorial`.

There is no hand-written prose anywhere in the generator output. Any future enrichment (GIFs, command-description columns, settings reference) must come from new auto-generated sources, not from special-case tutorial content.

## First-launch behaviour

The first launch dispatches `help.tutorial` automatically. The signal is the absence of a sentinel file at `%APPDATA%\continuity\.tutorial_seen` — `continuity_persist::tutorial_seen_path()` resolves the path; `app::main::take_first_launch_flag` checks existence and creates the sentinel on first read. Detection runs before window construction (in `build_initial_requests`); the flag is plumbed through `SpawnRequest::open_tutorial_on_init` → `WindowCommands::open_tutorial_on_init` → `Window::maybe_open_tutorial_on_first_launch`.

The sentinel is created eagerly (before the window opens) so a crash mid-launch never replays the tutorial open on the next run. The user can still invoke `help.tutorial` from the command palette at any time.

Deleting the sentinel re-arms first-launch behaviour — a small but explicit escape hatch.

## Tab semantics

The tutorial tab is a regular tab in the focused pane. It can be:
- closed via `tab.close` (or the close button), which doesn't drop the synthetic buffer from `EditorState` — same pattern as the metrics tab. Re-invoking `help.tutorial` then re-adopts the same buffer id as a fresh tab.
- focused via `help.tutorial` (idempotent: if a tab for the tutorial buffer is already open in any pane group, the command refocuses that tab rather than opening a second).
- searched (Ctrl+F) like any other buffer.

The synthetic buffer is **not** persisted, so it does not appear in:
- `find-in-all`
- quick-open
- the previous-buffer browser
- the time-machine slider

This falls out of the persist-skip guarantee — every consumer of those lists reads from persist, not from `EditorState`.

## Components

| Layer | File | Responsibility |
|---|---|---|
| `buffer` | `crates/buffer/src/buffer.rs` | `Buffer::synthetic_read_only(text)` constructor; `synthetic` + `read_only` flags; `Error::ReadOnly` variant. |
| `core` | `crates/core/src/handle.rs` | `AdoptBuffer` handler skips `persist.touch_buffer` when `buffer.is_synthetic()`. |
| `command` | `crates/command/src/help.rs` | `HELP_TUTORIAL` command id; `register_help_commands(registry)`; `TUTORIAL_MD` embedded asset. |
| `command` | `crates/command/src/view_context.rs` | `ViewContext::show_tutorial_buffer` trait method. |
| `command` | `crates/command/assets/tutorial.md` | The generated asset (do not edit by hand). |
| `ui` | `crates/ui/src/window_tutorial.rs` | `Window::show_tutorial_buffer_impl` (open-or-focus); `Window::maybe_open_tutorial_on_first_launch`. |
| `ui` | `crates/ui/src/window_view_options.rs` | `ViewOptions::tutorial_buffer_id` slot. |
| `ui` | `crates/ui/src/window_config.rs` | `WindowCommands::open_tutorial_on_init` flag. |
| `ui` | `crates/ui/src/window.rs` | First-launch hook in `Window::new`. |
| `app` | `crates/app/src/registry.rs` | `SpawnRequest::open_tutorial_on_init`; flag plumbed into `WindowCommands`. |
| `app` | `crates/app/src/main.rs` | `take_first_launch_flag` (sentinel check + write). |
| `persist` | `crates/persist/src/paths.rs` | `tutorial_seen_path()` helper. |
| `xtask` | `xtask/src/tutorial_gen.rs` | `cargo xtask gen-tutorial` + `check_drift()`. |
| `xtask` | `xtask/src/conventions.rs` | `conventions:tutorial-drift` rule. |

## Status

| Capability | State |
|---|---|
| Per-feature intros pulled from `.docs/design/features/*.md` | ✅ shipped |
| Hotkey table from `default.toml` | ✅ shipped |
| Command-reference appendix (id + description + binding + palette-safe) | ✅ shipped (via `continuity_command::default_registry`) |
| Settings-reference appendix (rustdoc-extracted) | ✅ shipped (parses `crates/config/src/`) |
| Feature-doc intro convention check (`conventions:feature-doc-intro`) | ✅ shipped |
| Animated GIF rendering | ✅ shipped (default 100 ms frame delay; cache stores all frames; auto-armed `WM_TIMER` advances frame_index) |
| Per-frame GIF delay from `/grctlext/Delay` metadata | ⏳ follow-up (currently uses `DEFAULT_FRAME_DELAY_MS = 100ms` for every frame; PROPVARIANT plumbing deserves a focused review) |
| Reduced-motion freeze on frame 0 | ⏳ follow-up (timer auto-arms even when reduced-motion is on; cheap follow-up: gate `ensure_image_animation_timer` on `view_options.<reduced_motion>` once a window-level reduced-motion flag is wired) |
| WEBM / video tutorial assets | ✗ out of scope (would need Media Foundation; static images + GIFs cover the use case) |
