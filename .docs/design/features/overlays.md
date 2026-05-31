# Overlays

Palette-mode surfaces: command palette, quick-open, find / find-and-replace, find-in-all, goto-line, goto-heading, font picker, theme picker, hex picker, tab switcher, and the slash palette. One state machine, one rendering surface, one dismiss path — every overlay is reversible and never traps input.

## What it is
- Transient input panels that preempt the editor body for keystrokes while open: command palette, find bar, find-in-all (legacy, retained for the framework), quick-open (legacy, deleted binding), goto-line, goto-heading. Each is one variant of `Overlays`. Keystroke dispatch routes them first (`overlay_on_char` / `overlay_on_keydown`) before the keymap fires.

## Key concepts
- **`Overlays`** — `enum { Idle, Find(FindBar), FindInAll(FindInAll), Palette(Palette), QuickOpen(QuickOpen), GotoLine(GotoLine), GotoHeading(GotoHeading) }`.
- **`Palette`** — command list of `{ label, command_id, applicable }`. Filter via command-label / command-id / alias fuzzy match with separator normalization (`space`, `_`, `.`, `-` all equivalent); `Enter` or row click dispatches the command by id; `Esc` or outside click dismisses.
- **`PaletteSession<M: PaletteMode>`** (Phase A1) — substrate for preview-while-hovering modes (font picker E3, theme picker E4, math eval E2, timeline I1, hold-modifier HUD E6). Guarantees one preview per highlight change, idempotent commit, drop-on-cancel.
- **`ViewOverlay`** (Phase A2) — per-pane transient override layer (font_family, theme_name, pinned_revision). Sits above settings, below persisted state. Used by E3 / E4 / I1 reveal-preview modes.
- **`FindBar`** — query / replacement state, mode flags, compact controls, match list, current index. Control hover renders command + hotkey rows; the regex control expands a helper list whose rows can insert common regex syntax.
- **`GotoLine`** — numeric target parser.
- **`GotoHeading`** — fuzzy filter over `Decorations::headings`.
- **Overlay input focus** — `Window::overlay_input_focused` tracks whether the visible overlay's text input owns keyboard focus. Clicking an overlay input focuses it; clicking a pane body blurs most overlay inputs and leaves the overlay visible while keystrokes return to the editor. The command palette is stricter: outside click dismisses it so the underlying UI click can continue. Cursor shape comes from overlay hit-testing: I-beam over text fields, hand over clickable controls / regex snippets / command-palette rows, arrow over panel background.
- **Passive hover-peek** — footnote hover uses the same `OverlayDraw` paint primitive but does not enter the `Overlays` enum and never owns keyboard focus. `Window::mouse_state.footnote_hover` is UI-thread state only; it starts a 300 ms dwell timer over a `SegmentHit::FootnoteReference`, paints the definition body, and clears on mouse-out or chord.
- **Overlay motion layer** — `Window::project_overlay_layer` wraps the current or dismissing `OverlayDraw` in the shared 160 ms ease-out-cubic fade/slide contract. Reduced motion projects the final static overlay and schedules zero frames.

## Dispatch order (`Window::on_keydown`)
1. Overlay text input focused → `overlay_on_keydown` consumes Enter / Esc / arrows / Tab / Backspace / Delete / Home / End / type chars.
2. If overlay declines AND chord is keymap-bound to a globally-active command (e.g. `Ctrl+F`, `F3`), dispatch via keymap.
3. If an overlay is visible but its input is blurred, buffer keymap dispatch resumes; the overlay remains visible until dismissed or re-focused.

Esc priority chain (Phase B3) runs only when no overlay is active (overlay's own Esc handler dismisses it). The chain is in `crates/ui/src/window_dismiss.rs`.

## Operations
- **Palette**:
  - `editor.show_palette` opens the palette; `refilter` runs command-palette ranking on every keystroke.
  - Empty filter ranks in-memory command recency first once populated; cold-start fallback is curated defaults (`file.open`, `editor.find`, `view.pick_theme`, `settings.open`, common view toggles), then alphabetical fallback.
  - Non-empty filter scores human labels, normalized command ids, descriptions, aliases, and compact forms so `pick theme`, `pick_theme`, and `picktheme` all reach `view.pick_theme`.
  - `Enter` confirms; row hover updates the selected row; row click confirms. Only `applicable` rows dispatch `command_id` against the window context.
  - Results are capped to 10 visible rows. Overflow stays in the palette state as a scroll window, with mouse wheel support over the panel and a vertical scrollbar in the overlay draw payload.
- **Slash palette** (Phase H5, separate `Overlays::SlashPalette` variant):
  - Typed-`/` line-start trigger and the explicit `Ctrl+/` chord (`view.slash_palette_show`) both route through `Window::show_slash_palette_impl`. The trigger origin (`SlashTrigger::TypedSlash` vs `ExplicitChord`) is recorded on the overlay state so the Esc cleanup path knows whether to delete the literal `/` from the rope.
  - Populator: `Registry::palette_safe_ids` by default; `editor.slash_commands_palette` (when `Some`) replaces it verbatim, in that order.
  - **Backspace** at zero filter chars dismisses the palette and leaves the `/` in source.
  - **Esc** dismisses; for typed-`/` triggers, the trailing `/` is removed via the standard `delete_back_at_selections` path.
  - **Enter** dispatches the selected command id (the inserter writes the markdown; the palette never edits the rope itself beyond the `/` cleanup).
- **Find bar**: see [`search.md`](search.md).
- **Goto-line**: numeric input only; `target() -> Option<(line: u32, col: u32)>`; `Enter` confirms; B6 glow fires if the jump distance > threshold.
- **Goto-heading**: populates from the active buffer's `Decorations::headings`; `selected_entry()` returns the chosen `HeadingEntry`; `Enter` jumps to `(entry.line, 0)`; B6 glow fires.
- **Footnote hover-peek**: `Window::update_footnote_hover_from_pixel` resolves the hovered source byte through the current decoration snapshot, extracts the matching definition body from the rope, and arms `FOOTNOTE_HOVER_TIMER_ID`. `Window::footnote_hover_overlay` emits a small passive `OverlayDraw`; `clear_footnote_hover` dismisses on mouse-out, Esc priority-chain, or non-Esc chord.
- **Banner / overlay / chord HUD motion**: visible overlay, file-banner, passive chord HUD, and dismissing overlay frames share `SurfaceMotionState`. The renderer receives immutable `SurfaceMotion` in `DrawParams`; timers and stagger remain UI-thread state on `Window`.

## Key state and helpers
- `Overlays::find_bar()` / `_mut()`, `palette_mut()`, `quick_open_mut()`, `goto_line_mut()`, `goto_heading_mut()`.
- `Overlays::is_active() / dismiss()`.
- `Window::populate_goto_heading` rebuilds the heading list from the latest decoration snapshot.

## API surface
- Public from `crates/ui/src/`:
  - `overlays.rs::{Overlays, OverlayKind}`
  - `palette.rs::{Palette, PaletteEntry}`
  - `palette_mode.rs::{PaletteMode, PaletteSession, PaletteRow}`
  - `view_overlay.rs::ViewOverlay`
  - `find_bar.rs::{FindBar, FindFocus}`
  - `find_regex_help.rs::{FindControl, REGEX_SNIPPETS}`
  - `find_in_all.rs::{FindInAll, FlatRow}` (legacy, kept for the framework)
  - `quick_open.rs::QuickOpen` (legacy framework retained; binding removed per D9)
  - `goto_overlay.rs::{GotoLine, GotoHeading}`
  - `slash_palette.rs::{SlashPalette, SlashPaletteEntry, SlashTrigger}` (H5)
  - `tab_switcher.rs::{TabSwitcher, TabSwitcherRow}` (H6)
  - `overlay_render.rs::layout_*` paint helpers; focused input fields carry caret and selection ranges into `crates/render/src/overlay.rs`
  - `overlay_render_find.rs` find-bar control layout, hit-testing, tooltip rows, and regex helper rows.
  - `overlay_render_palette.rs` command-palette capped-row layout and scrollbar geometry.
  - `palette_rank.rs` command-palette defaults, aliases, and separator-normalized ranking.
- Window-side: `crates/ui/src/window_overlays.rs`, `window_overlay_input.rs`, `window_search.rs`, `window_find_replace.rs`.

## Configuration
- `editor.slash_commands_enabled` (default `true`).
- `editor.slash_commands_palette` — user-overridable safelist (Phase H5).

## Key files
- overlays enum: `crates/ui/src/overlays.rs`
- palette: `crates/ui/src/palette.rs`, `palette_mode.rs`
- view overlay: `crates/ui/src/view_overlay.rs`
- find bar: `crates/ui/src/find_bar.rs`
- goto: `crates/ui/src/goto_overlay.rs`
- text input field: `crates/ui/src/text_input.rs`
- paint: `crates/ui/src/overlay_render.rs`, `crates/ui/src/overlay_render_find.rs`, `crates/ui/src/overlay_render_palette.rs`, `crates/render/src/overlay.rs`, `crates/render/src/overlay_scrollbar.rs`
- footnote hover-peek: `crates/ui/src/footnote_hover.rs`, `crates/ui/src/window_footnote_hover.rs`
- motion projection: `crates/ui/src/surface_motion.rs`, `crates/ui/src/window_motion.rs`, `crates/render/src/overlay_motion.rs`
- passive chord HUD: `crates/ui/src/chord_hud_render.rs`
- Window dispatch: `crates/ui/src/window_overlays.rs`
- Esc dismiss chain: `crates/ui/src/window_dismiss.rs`

## Relates to
- [Command system](command-system.md) — palette confirm dispatches command ids; slash palette filters on `palette_safe`.
- [Search](search.md) — find bar + replace bar + goto-line + goto-heading.
- [Decoration](decoration.md) — heading list comes from `Decorations`.
- [Caret presentation](caret.md) — confirm-jump fires B6 glow + B7 tween.
- [Motion](../motion.md) — overlay, banner, and chord-HUD open/close timing plus reduced-motion behavior.
