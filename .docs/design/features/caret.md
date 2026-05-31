# Caret presentation

Caret rendering: shape (bar / block / underline), blink behaviour, jump-glow acknowledgement, motion tween on large jumps, and sticky-column tracking through vertical motion. Per-pane state; reduced-motion honoured.

## What it is
- Per-pane caret rendering + animation state. Shape (`bar` / `block` / `underline`), bar width, blink interval, type-pause suppression, jump-glow on long motions, motion-tween on edit-driven jumps, sticky column across vertical motion. All settings hot-reloadable.

## Key concepts
- **`CaretShape`** — `Bar | Block | Underline`. Renderer input; `chrome_caret::caret_rect_for_shape(..., bar_width_px)` computes the D2D rect.
- **`ViewOptions` (per pane)** — carries the renderer-facing caret config: `caret_style`, `caret_blink_ms`, `caret_width_px`, `caret_blink_on_typing_pause`, `caret_typing_pause_ms`, `caret_color`, `caret_secondary_color`, `caret_tween_enabled`, `caret_tween_threshold_rows`, `caret_tween_duration_ms`.
- **Blink state on `Window`** — `caret_blink_visible`, `caret_blink_active`, `last_input_tick`. Driven by a `WM_TIMER` (CARET_BLINK_TIMER_ID) on the blink period.
- **`intended_columns: Vec<u32>` + `intended_display_columns: Vec<u32>` + `intended_columns_for: Vec<Position>`** — Phase B2 sticky column for vertical motion. Two parallel sticky columns (source-byte for the non-wrap path, display-byte within the head's wrapped row for the soft-wrap path) keyed by the same fingerprint. Any horizontal motion / edit perturbs the fingerprint and the next vertical step reseeds both from live values; sequential up/down keeps the same column even through narrower rows (wrapped or not).
- **`JumpGlow { line, started_ms }`** — Phase B6 acknowledgement glow on long caret jumps. Fade follows the shared 180 ms ease-out-cubic motion contract.
- **`CaretTween { from_line, to_line, started_ms, duration_ms }`** — Phase B7 motion tween over the shared 160 ms ease-out-cubic structural duration.

## Operations

### Screen-y anchor across reflow (δ.3)
The caret's display-line screen y is preserved across every reflow source — font scale, font family, soft-wrap toggle, viewport width/height changes, pane geometry, distraction-free toggle. The contract is documented in `.docs/design/principles.md` §"Layout shifts preserve caret-line screen y"; the audit + remediation history is at `.docs/development/archive/audit_caret_anchor.md`.

Implementation: `crates/ui/src/window_caret_anchor.rs::Window::with_caret_line_anchored<F, R>(f: F) -> R`. Wraps any closure that mutates view geometry; captures the caret's `(source-position, display-line screen y)` pair before, recomputes the caret's display-line index under the post-reflow `FrameDisplay`, and adjusts `view.scroll_y_dip` so the caret line lands at the snapshotted y. Pure math factored as `anchored_scroll(..)` with unit-test coverage.

Single funnel: `window_panes.rs::refresh_focused_viewport` is wrapped, so every caller (WM_SIZE, pane resize, sidebar toggle, minimap appearance, distraction-free) inherits the anchor. Font-state callers wrap directly because the mutation happens before `invalidate_font_state` is invoked.

Future reflow-causing call sites must route through `with_caret_line_anchored`. Never write parallel anchor logic.

**Live drag-resize exception.** Inside a Win32 modal sizing loop (`WM_ENTERSIZEMOVE` → `WM_EXITSIZEMOVE`), `refresh_client_size` takes a cheap unanchored path (`refresh_focused_viewport_unanchored`) on every per-tick `WM_SIZE`. The per-tick anchor build runs a full `FrameDisplay` projection and dominates resize CPU. A single anchor is captured at `WM_ENTERSIZEMOVE` (via `capture_caret_anchor`) and restored once at `WM_EXITSIZEMOVE` against the final projection (via `restore_caret_anchor`), so the screen-y contract still holds for the final frame the user actually settles on. Intermediate frames during the drag are explicitly *not* anchored — that's the entire point of the optimisation, and it matches the perceptual fact that the user's eye is tracking the resize handle, not the caret line. The path remains anchored for every non-live caller (font change, soft-wrap toggle, pane resize commands, sidebar toggles, distraction-free, etc.). State: `Window::{is_live_resizing, resize_anchor, resize_changed}`.

**View-reset short-circuit.** `refresh_focused_viewport` skips `with_caret_line_anchored` whenever `view.viewport_*_dip == 0` — every caller that resets the per-pane `ViewState` (`switch_focus` no-saved-state branch, `open_new_tab`, `split`, `adopt_buffer_as_new_tab`, `reopen_closed_tab`, `apply_layout_shortcut`) leaves `scroll_y_dip = 0`, so the anchor's "preserve old screen y" semantic is inapplicable. Skipping avoids unnecessary row-index work at a new wrap and keeps clicks into never-painted panes or layout-shortcut keypresses on the unanchored reset path. Trace label `refresh_focused_viewport source=unanchored_view_reset` names the skip. Future reset-then-refresh callers automatically benefit because the guard is centralised in the helper. See `panes-tabs-windows.md` § "Focus-switch and layout-shortcut anchor short-circuit".

### Sticky vertical column (B2)
`Window::move_line_selection(delta, extend)`:
1. Take a snapshot of selections.
2. Compute a fingerprint = current head positions.
3. Build a per-call `FrameDisplay` when soft-wrap is on (so motion steps by display rows instead of source lines).
4. If `intended_columns_for == prev_heads`, reuse the stored sticky columns. Otherwise reseed both `intended_columns` (from `head.byte_in_line`) and `intended_display_columns` (from `head_display_byte_in_row`).
5. For each selection, compute `new_head` — `move_line_with_column` in the non-wrap path, `move_visual_row` with the stored display-byte target in the wrap path.
6. Update `intended_columns_for = new_selections.heads`.

The pure function `selection_vertical::move_line_with_column` is unit-tested headless — clips to EOL on short lines but the column memory survives so a wider next line restores the original column. The same invariant holds for `move_visual_row` in the soft-wrap path: the stored display-byte target survives narrow rows, so `Up` through an empty wrapped row onto a wider one restores the original column.

### Blink (B5)
`on_caret_blink_tick`:
1. Evict expired jump_glow + caret_tween opportunistically; active motion is driven by `MOTION_TIMER_ID`.
2. If `caret_blink_on_typing_pause` AND `now - last_input_tick < caret_typing_pause_ms`, keep `caret_blink_visible = true`; return.
3. Otherwise toggle visibility.

`note_input_now()` is called from `on_keydown`, `on_char`, and `Context::apply_selection_edit` (Phase B5) so every edit / keystroke / paste / undo path keeps the caret solid until the pause elapses.

### Jump glow (B6)
- `should_glow(from_line: Option<u32>, to_line, threshold)` — `true` when `|to - from| > threshold` or `from_line == None` (cross-buffer jump).
- `fade_alpha(glow, now, fade_ms) -> Option<f32>` — ease-out-cubic from 1.0 to 0.0; `None` when done.
- Hooks: `Window::capture_caret_line_for_jump` + `maybe_trigger_jump_glow(from_line)`. Called from `confirm_goto_line`, `confirm_goto_heading`, `jump_to_current_find_match`, `confirm_quick_open` (`None` for cross-buffer).
- Reduced motion clears the glow and does not arm the motion timer.

### Vertical autoscroll while drag-selecting

When the user drags a text selection past the focused pane's body
edge, a 16 ms `WM_TIMER` (text-selection autoscroll) extends the
selection at the clamped body edge and scrolls in the cursor's
direction. Implemented in `crates/ui/src/window_mouse_autoscroll.rs`;
state lives on `mouse::Autoscroll { last_cursor_x, last_cursor_y,
direction: Up|Down, distance_dip, started_ms }`.

Invariants:
- Autoscroll arms only for a drag started inside the focused pane's
  body, and only while that same pane stays focused.
- Sits behind existing drag owners — time-machine slider, scrollbar
  drag, splitter, and tab drag all return before the selection branch
  and the timer eligibility check repeats those guards.
- Body rect is re-read on every move and timer tick, so pane resize
  during drag uses current geometry.
- Stop conditions: button-up, `WM_CAPTURECHANGED`, cursor returning
  inside the body / dead band, scroll clamp at the document edge.
- Cursor x/y are clamped to the live body before extension; the same
  `place_caret_at_pixel(..., extend=true)` path as `WM_MOUSEMOVE` runs.
- Reduced motion uses the same speed curve rounded to whole-line
  steps per tick.

Trace event: `event:mouse_drag_autoscroll state=start|tick|stop
direction=up|down distance_dip=<i32> lines_advanced=<i32>
reason=edge_exit|capture_lost|buffer_end|button_up
elapsed_ms_since_start=<u32>`.

Horizontal autoscroll is intentionally out of scope today.

### Edit pulse (α.1)
- `EditPulse { first_line, last_line, started_ms, duration_ms, kind }` lives in `crates/ui/src/edit_pulse.rs`. One `Option<EditPulse>` slot on `Window` — most recent wins, same shape as jump-glow.
- Three kinds share the mechanism:
  - **`EditRegion`** — paste, duplicate-line, move-line, sort/reverse/unique, surround, transpose, markdown emphasis/heading/section/list, and every other structural `SelectionEdit`. 120 ms. Fired from `selection_dispatch::Window::dispatch_selection_edit` (via `is_structural_edit`) and `window_clipboard::{paste_clipboard_impl, paste_from_history_impl}` (paste flows through `SelectionEdit::InsertText` so it arms explicitly).
  - **`UndoTarget`** — fired from `window_commanding::{undo, redo, redo_alternate_branch, undo_tree_pick}` against the post-undo selection's row range. Same 120 ms window.
  - **`SelectionExpand`** — fired from `selection::Window::expand_selection_smart` when the smart-expansion step moved a selection. 80 ms, the shorter window so the ladder reads as tactile rather than as a structural pulse.
- `edit_pulse_range(pre_caret_line, pre_line_count, post_sel, post_line_count) -> (u32, u32)` bridges the three input shapes: caret-moves-with-content (paste, MoveLineDown), caret-stays-with-content-below (DuplicateLine — widen by `len_lines` delta), and caret-stays-after-delete (collapse to caret line).
- `is_structural_edit(&SelectionEdit) -> bool` is the gate: continuous-typing inserts, single-key deletes, indent/outdent, auto-pair, and whole-buffer normalisations stay silent so a 60 WPM run doesn't strobe the screen.
- Render side: `EditPulseDraw { first_line, last_line, alpha, color }` and `paint_edit_pulse` paint a flat-alpha tint band across the affected source rows, after the body and any jump glow.
- Theme key: `editor.edit_pulse` (low-alpha blue across bundled themes).
- Reduced motion: `trigger_edit_pulse` clears any active pulse and returns without arming the timer; `apply_reduced_motion` also clears `edit_pulse` alongside `jump_glow` / `caret_tween`.

### Motion tween (B7)
- `should_tween(enabled, from, to, threshold)` — `true` when enabled AND `|to - from| > threshold`.
- `tween_progress(tween, now)` — ease-out cubic; `None` when finished.
- `interpolated_line(tween, progress)` — linear lerp between rows consumed by the renderer during paint.
- Hooks: `Window::maybe_start_caret_tween(from_line)` from `jump_to_current_find_match`. Reduced motion clears the tween and produces no animation frames.

### Width + color
- Bar width: `view_options.caret_width_px` (default 2 DIP). `caret_rect_for_shape(_, _, _, _, shape, bar_width_px)` consumes it. `0` falls back to the legacy 1.5 DIP.
- Color override: `view_options.caret_color` / `caret_secondary_color`. Accepts `#rrggbb` / `#rrggbbaa` hex or a dotted theme-key reference. Empty string falls through to the theme's `editor.cursor.primary` / `secondary`. Paint integration at the brush layer is a follow-up (renderer still uses theme keys).

## Configuration
- All `editor.caret_*` settings from `[editor]` (see `settings.md`).
- `[ui].reduced_motion` disables caret tween and jump-glow animation per [`motion.md`](../motion.md).
- All hot-reloadable.

## Key files
- caret rect: `crates/render/src/chrome_caret.rs`
- blink + run-loop timers: `crates/ui/src/window_runtime.rs`
- sticky column: `crates/ui/src/selection.rs`, `crates/ui/src/selection_vertical.rs`
- jump glow: `crates/ui/src/jump_glow.rs`
- motion tween: `crates/ui/src/caret_tween.rs`
- shared contract: `crates/ui/src/motion.rs`, `.docs/design/motion.md`
- view options: `crates/ui/src/window_view_options.rs::ViewOptions`
- settings → view-options apply: `crates/ui/src/window_settings_reload.rs::apply_settings`
- params plumbing: `crates/render/src/params.rs::ViewOptionsDraw`

## Relates to
- [Selections + edits](selection-edits.md) — every edit fires `note_input_now()` keeping the caret solid; tween hooks live alongside.
- [Motion](../motion.md) — canonical duration, easing, reduced-motion, and zero-frame contract.
- [Settings](settings.md) — all caret config flows through `[editor]`.
- [Search](search.md) — find-jump triggers glow + tween.
- [Overlays](overlays.md) — goto-line / goto-heading / quick-open confirm triggers glow.
