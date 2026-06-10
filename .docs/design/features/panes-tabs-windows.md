# Panes, tabs, windows

Recursive pane tree inside each window; positional tabs per pane leaf with an MRU step via Ctrl+Tab. Panes split / close / focus-move; tabs reorder, pin, close, tear off to a new window. The full pane / tab / window tree restores across sessions, including virtual-desktop placement on Windows 10+.

## What it is
- Multiple top-level Win32 windows, each with an arbitrary pane tree (N-way splits, nested), each leaf a tab `Group` with MRU + positional order. Virtual-desktop GUID per window is captured at save time so cross-desktop layouts restore. No global hotkeys; per-window keymaps only.

## Key concepts
- **`Window`** — one top-level Win32 window. Owns: HWND, swap chain, `PaneTree`, focused-pane scalar mirrors (`view`, `buffer_id`, `language`), per-pane `panes: HashMap<PaneId, PerPaneState>`, buffer-local right-edge chrome overrides (`right_edge_chrome_by_view`), view options, theme, paste history, IME state, spell state.
- **`PaneTree`** — `Split { axis, ratio, children: [PaneNode; 2..] } | Group { tabs: Vec<TabId>, active: TabId, mru: Vec<TabId> }`. Fully general; the UI exposes shape shortcuts (`Alt+Shift+1/2/3` for single / two-col / 2×2 grid; legacy `Ctrl+Alt+1..8` aliases).
- **`Group`** — leaf pane; holds tabs in positional + MRU orders.
- **`Tab { id, buffer_id, created_at_ms, label_override }`** — one tab.
- **`PerPaneState`** — non-focused pane runtime state (buffer id, view, language, decoration revision). The focused pane mirrors into the scalar window fields.
- **Pane/tab chrome motion** — `ChromeMotionState` compares focused pane and active tab ids during paint and annotates `PaneStripDraw` with the shared 160 ms ease-out-cubic focus/activation motion. Focus changes crossfade *both* directions on the same `MotionSpan`: the focus-in pane carries `focus_motion` with a rising active-border α, the focus-out pane carries the complementary falling α, and the renderer blends `pane.border` ↔ `pane.border_active` (plus border thickness) by that value regardless of the static `focused` flag. Tab activation projects an `active_tab_motion` progress plus `previous_active_tab_index` onto the changing pane; the renderer paints a 3 DIP underline (`active_tab_fg` color) lerped from the previous-active tab's rect to the current-active tab's rect along the strip's x-axis. Reduced motion clears all three projections and schedules no frames.
- **Virtual desktop** — `IVirtualDesktopManager` (COM) captured at state-save time; restored on launch by GUID. Missing desktop ⇒ fall back to active desktop.

## Pane-tree integrity and live-buffer invariant

Every visible leaf must have a `Group`; every group must have at least one tab;
`Group::active` and every MRU entry must be a tab in that same group; and every
`TabKind::Buffer` tab must point at a `BufferId` with a live core snapshot
before focus, layout, or paint can treat it as active. Runtime paths are allowed
to use direct `groups[focused]` / `tabs[active]` lookups only after those
invariants have been checked.

Persisted pane-tree JSON is decoded strictly: empty groups, duplicate tab
assignment, active tabs outside the group, MRU entries outside the group, root
leaves missing group payloads, and orphan tab payloads are rejected so restore
falls back cleanly instead of carrying a blank pane into runtime. The UI thread
also runs `window_buffer_tab_repair` after pane-tree restore and before
layout/focus/tab-close/tab-adoption paths. That repair preserves the visible
layout where possible, replaces unusable leaves with a fresh empty buffer tab
via `EditorHandle::open_buffer`, rewrites only that window's `PaneTree`, and
leaves core as the sole writer of buffer text.

Trace events are emitted only when `CONTINUITY_UI_TRACE` is enabled:
`pane_tree_structure_repair reason=… repairs=…` for structural normalization
and `pane_tree_buffer_repair tab=… old_buffer=… replacement_buffer=…` for a
missing core snapshot. A trace that still shows `active_tab=missing` or
`projection_worker_early_dispatch … skip=skip_no_snapshot` after a focus switch
now means a new coherence path was missed, not an expected cache state.

## Window focus + caption

- **Two focus flags**, both UI-thread-owned on `Window`: `is_window_focused` (the app-level foreground flag, toggled on `WM_ACTIVATEAPP`) and `has_keyboard_focus` (this HWND holds keyboard focus, toggled on `WM_SETFOCUS` / `WM_KILLFOCUS`). The second exists because switching between two continuity windows in the same process fires the focus messages but **not** `WM_ACTIVATEAPP`.
- **Active-pane highlight requires focus.** `build_pane_chrome` flags a leaf `focused` only when it is the tree's focused pane **and** `is_window_focused` **and** `has_keyboard_focus`. A background window (other app focused, or another continuity window focused) paints no active-pane border.
- **Focus-loss input reset.** On `WM_KILLFOCUS` and `WM_ACTIVATEAPP(false)`, `on_focus_lost_clear_input_state` clears held-modifier-derived UI state — the hold-modifier chord HUD, any pending chord leader, and the Ctrl+Tab tab-switcher chord — because the modifier key-up that would normally clear them is delivered to the newly focused window. Without this an alt-tab away leaves the hotkey HUD pinned "as if Alt were held". Lives in `crates/ui/src/window_focus.rs`.
- **Window caption mirrors the active tab.** `sync_window_title` (in `window_titlebar.rs`, called each paint, guarded so the `SetWindowTextW` syscall only fires when the text changed) sets the OS caption / taskbar / Alt-Tab label to the active tab's resolved label (or `Untitled`), instead of a static `"continuity"`.

## Data model

```rs
enum PaneNode { Split { axis: SplitAxis, ratio: f32, children: Vec<PaneNode> },
                Group { tabs: Vec<TabId>, active: TabId, mru: Vec<TabId> } }
enum SplitAxis { Horizontal, Vertical }

struct ClosedTab {
    buffer_id:           BufferId,        // the closed tab's buffer
    label:               String,          // resolved label at close time
    closed_at_ms:        u64,
    origin_pane:         Option<PaneId>,  // pane that hosted the tab (None for legacy records)
    parent_split_axis:   Option<SplitAxis>,
    parent_sibling_leaf: Option<PaneId>,  // first sibling leaf at close time
}
```

Persisted to `panes` + `tabs` + `view_states` tables (see [`data_model.md`](../data_model.md)). `recently_closed: Vec<ClosedTab>` is per-window in-memory but round-trips through the `pane_tree_json` blob (legacy records decode with the three trailing `Option` fields `None`).

### Close-reopen history

Two stacks back `Ctrl+Shift+T`:

- **Per-window `PaneTree::recently_closed`** — single closed tabs, cap 32. Constructed in `Window::close_active_tab` and `pane_layout::close_pane` (cascade).
- **Global `closed_history` SQLite table** — whole-window closes, pushed by `archive_closed_window` before the window row is tombstoned. Bounded by `STACK_CAP`; older entries evict silently.

`smart_reopen_handler` picks between them by comparing the local top's `closed_at_ms` against the global top's `closed_at_ms`. The smart handler is **transactional**: it peeks the global row, dispatches the spawn, and only pops on `tx.send` success.

## Operations
- **Layouts** (commands build the tree shape, preserve the focused tab in the top-left leaf, distribute other tabs round-robin):
  - `Alt+Shift+1` single (`pane.layout_single`)
  - `Alt+Shift+2` two columns (`pane.layout_two_col`)
  - `Alt+Shift+3` 2×2 grid (`pane.layout_grid_2x2` — Phase E7)
  - legacy `Ctrl+Alt+{1..8}` aliases retained (spec §6 plus E8 delta)
- **Split**: split focused pane horizontal / vertical.
- **Focus**: geometric `focus_left/right/up/down` (not tree-order). Falls back to the closest pane in the requested direction.
- **Wheel hover routing**: plain vertical wheel input scrolls the pane
  body under the cursor without changing focus. Pane body chrome
  (gutter, line numbers, minimap, outline/sidebar chrome, scrollbar)
  still belongs to that pane; tab strips, status/title surfaces, points
  outside this window's client rect, active overlays, and active drags
  claim or suppress the wheel before pane routing.
- **Right-edge chrome visibility**: minimap and outline visibility are
  keyed by active `BufferId`. `[ui].show_minimap` and
  `[ui].show_outline_sidebar` are defaults for buffers without runtime
  overrides; `view.toggle_minimap` / `view.toggle_outline` record the
  focused buffer's override. Non-focused panes paint and wrap using
  their active buffer's flags, so split panes can show different
  right-edge chrome. Right-click over file tree / minimap / outline
  chrome opens a one-item toggle menu; right-click over non-focused
  minimap or outline chrome focuses that pane before dispatching the
  toggle command.
- **Focus motion**: focused-pane border crossfades under the shared motion contract; simultaneous focus/tab changes use the Window `StaggerScheduler`.
- **Tabs**:
  - `Ctrl+Tab` / `Ctrl+Shift+Tab` — **positional** order (Phase H6; spec §6 override §L#1). MRU traversal moved to `Ctrl+Alt+Tab`.
  - `Ctrl+PageUp/PageDown` — positional.
  - `Ctrl+1..9` — jump to positional 1..9.
  - drag-reorder; drag onto another pane's strip → move; `Ctrl+drag` clones (Phase D2).
- **Tab activation motion**: active-tab accent is annotated during paint rather than stored in the pane tree. This keeps persistence synchronous and leaves all mutable motion state on the UI thread.
- **Ctrl+Tab transient overlay** (Phase H6): a sustained `Ctrl` hold past 600 ms during a `Ctrl+Tab` chord opens a positional tab-switcher overlay. Single-tap releases (< 600 ms) skip the overlay entirely and fall through to the synchronous positional swap — the "fast swap, no flicker" path. While the overlay is visible, additional `Ctrl+Tab` taps step the highlight cursor and preview the corresponding tab in the focused pane *without* mutating the MRU stack (`Group::set_active_for_preview` swaps `active` only). `Esc` reverts to the tab that was focused when the overlay opened. Releasing `Ctrl` commits the highlighted tab (promoting it to MRU front via `Group::activate`) and dismisses the overlay. Implementation: state in `crates/ui/src/tab_switcher.rs`; chord state machine + 600 ms `WM_TIMER` in `crates/ui/src/window_tab_overlay.rs`; `WM_KEYUP` Ctrl dispatch in `window_commanding.rs::on_keyup`; paint in `overlay_render_pickers.rs::layout_tab_switcher`.
- **Tear-off**: drag a tab outside any pane strip → new window with that tab. Mouse tear-off passes the release point in screen pixels through `window.tear_off_focused_tab`; the registry uses that point as the new window's initial outer top-left instead of the normal cascade offset. Keyboard/menu tear-off still cascades from the source window.
- **Drag-in-flight visual feedback**: the four drop resolutions are
  resolved once per move by a pure helper
  (`Window::compute_tab_drop_resolution`); both preview (`WM_MOUSEMOVE`)
  and commit (`WM_LBUTTONUP`) call the same helper with the same cursor,
  so the painted affordance can never disagree with the commit. The
  `TabDropResolution` enum is exhaustive
  (`Cancel | SourceStrip | PaneBody | ForeignWindow | TearOff`). Target
  affordances project through `TabDragOverlayDraw`; the cursor-attached
  tab replica is a separate no-activate popup owned by the source window
  so it stays visible outside the source HWND:
  - **`SourceStrip`** — 2-DIP insertion bar (`pane.border_active`) at
    the slot boundary; source tab fades 120 ms ease-out toward 60 %.
  - **`PaneBody`** — pane-body tint (6 % accent + 2-DIP border, inset
    4 DIP from body edges).
  - **`ForeignWindow`** — cross-window broadcast (see below).
  - **`TearOff`** — desktop / other-app / chrome release; commit spawns
    at the release point when the mouse supplied one.
  - **Tab ghost** — `Window::tab_drag_ghost_window` mirrors the dragged
    tab's active-tab fill, foreground, close glyph visibility, strip
    height, slot-width calculation, and active border color. Created on
    tab-drag start, moved on every drag mousemove in screen pixels, and
    destroyed on drop / cancel / capture loss.
- **Cross-window foreign-side indicators**: the source window broadcasts
  hover state via the lazily-registered `Continuity.TabDragHover`
  Win32 message (`RegisterWindowMessageW`) on every `WM_MOUSEMOVE`
  while the resolution is `ForeignWindow`. The receiver's
  `on_foreign_tab_drag_hover` classifies the broadcast cursor among
  `Strip / Body / chrome-fallback` and paints the matching indicator
  (insertion bar for strip; pane-body highlight for body; focused
  pane's body for chrome-fallback). The source-side tab replica stays
  visible as a no-activate popup while the foreign window paints its
  target indicator. The broadcast reuses the same registry walk + DPI
  conversion as cross-window adoption so preview and commit see the
  same sibling. Hysteresis const lives in
  `window_tab_drag::TAB_DRAG_HYSTERESIS_PX`.
- **ESC / capture loss**: `cancel_tab_drag` runs through the dismiss
  priority chain (`window_dismiss.rs`) before any other dismissal and
  is also invoked from `WM_CAPTURECHANGED`. The cancel path broadcasts
  a hover-leave so the foreign overlay clears immediately.
- **Trace event**: `event:tab_drag state=start|over|drop|cancel
  target=cancel|source_strip|pane_body|foreign_window|tear_off
  slot=<i32 or -1> foreign_hwnd=<u64 or 0> elapsed_ms_since_start=<u32>`.
  `over` emits only on variant or key-payload change, never per move.
- **Close**:
  - `[ui].tab_close_button = "hover"` paints the close glyph only for
    the tab currently under the pointer. The same UI-thread hover slot
    gates close-glyph hit-testing, so an unpainted close cell cannot
    close a tab.
  - `Ctrl+W` closes a tab (buffer → trash if not file-associated; just closes if file-associated).
  - `Ctrl+Shift+W` closes a pane.
  - Closing the last tab in a pane closes the pane (D4); closing the last pane closes the window; closing the last window saves session state and quits (`AppSessionEnding`).
  - Every closed tab pushes a [`ClosedTab`](#close-reopen-history) record onto the window's `recently_closed` stack (cap 32) capturing `buffer_id`, label, close timestamp, the originating pane id, **and** the parent split's axis + first sibling leaf — the shape hints reopen needs to restore the pane when this close collapses it. Pane-collapse cascades (`pane_layout::close_pane`) push one record per tab in the collapsing pane.
- **Reopen-closed** (`Ctrl+Shift+T`):
  - The smart handler (`crates/app/src/registry_closed_history.rs::smart_reopen_handler`) picks between the in-window `recently_closed` stack and the global `closed_history` table (whole-window closes). Newer close timestamp wins; ties favor the global stack.
  - **Local path** (`Window::reopen_closed_tab`): pops the stack head, but probes `EditorHandle::snapshot(buffer_id)` first — `None` means the buffer is no longer adopted, the entry is skipped, and the loop tries the next one. No phantom tabs are installed.
  - **Destination routing** on a live entry, in priority order:
    1. **Origin pane alive** → install tab in the originating pane.
    2. **Origin collapsed, sibling+axis recorded, sibling still a leaf** → mint a fresh pane id and re-split the recorded sibling along the recorded axis via `pane_layout::splice_split_at_pane`. The reopened tab lands in the new pane, which approximates the original tree position.
    3. **Otherwise** → install in the currently focused pane.
  - **Global path** (whole-window restore): the handler peeks the `closed_history` row, dispatches a `RegistryEvent::Spawn` reconstructing the window from the persisted pane-tree JSON, and only then pops the row. If `tx.send` fails, the row stays on the stack so the next press can retry.
  - Trace events: `event:tab_close`, `event:pane_close`, `event:tab_reopen outcome=… dest=…`, `event:smart_reopen outcome=…`, `event:closed_history_push/pop`. See [`trace-guide.md`](../../technical/trace-guide.md) "User actions + lifecycle".
- **Tab labels** (Phase B15): user override → first non-empty trimmed line with leading `#`s stripped → `Untitled`. Derived labels are clipped to 20 chars with `…`; renderer chrome keeps the label one-line and clipped inside its tab slot. The same resolved label drives the OS window caption (see § Window focus + caption).
- **Window placement**: `GetWindowPlacement` / `SetWindowPlacement` persists position; handles minimized / maximized / off-screen reposition across monitor reconfig. `WM_MOVE` triggers `request_state_save` (Phase A8).
- **Single instance per data dir**: `app::single_instance` + `win::single_instance` hold a named mutex keyed by the database path. A second launch forwards its command-line paths to the running instance over a `WM_COPYDATA` message-only hub and exits (bare launch → activate top-most window); only when no instance is reachable does it run standalone. `--new-instance` bypasses. This prevents a second launch from replaying the whole persisted session and duplicating every window. See `architecture.md` § Process model and `defaults.md` § Launch + sessions.
- **Restore activation gating**: `SpawnRequest.activate_on_restore` (set only on the most-recently-seen window at launch) → `WindowConfig.activate_on_show` → `Window::run` shows non-activating restored windows with `SW_SHOWNOACTIVATE`. `apply_initial_placement` additionally forces `activate_on_show = false` when the window restores onto a non-active virtual desktop, so launch never switches the user's desktop. Runtime spawns and `Ctrl+Shift+T` reopen activate normally.
- **Virtual desktop** (Phase 14):
  - `IVirtualDesktopManager::GetWindowDesktopId(hwnd)` captures the GUID at save time.
  - `MoveWindowToDesktop` restores on launch.
  - Missing desktop ⇒ fall back to active desktop, no auto-switch.

## State saves (Phase A8)
`request_state_save` is wired into every state-change event with a 250 ms debounce:
- focus switch (`switch_focus`)
- tab walk (`step_tab_positional`, `activate_positional_tab`, `step_tab_mru`)
- pane split/move/close, tab move-between-panes, close-active-tab, reopen-closed
- view changes: zoom adjust/reset, soft-wrap toggle, scroll
- window move (`WM_MOVE`)

Virtual-desktop GUID is captured at save time via `current_desktop_guid`; no separate trigger needed.

## Focus-switch and layout-shortcut anchor short-circuit

`Window::refresh_focused_viewport` skips `with_caret_line_anchored` whenever `view.viewport_*_dip == 0` — every caller that resets the per-pane `ViewState` (`switch_focus` no-saved-state branch, `open_new_tab`, `split`, `adopt_buffer_as_new_tab`, `reopen_closed_tab`, `apply_layout_shortcut`) leaves `scroll_y_dip = 0`, so there is no prior screen y to preserve. With the anchor's "preserve old screen y" semantic inapplicable, the helper falls through to `refresh_focused_viewport_unanchored`. This avoids unnecessary row-index work at a new wrap and keeps clicks into never-painted panes or layout-shortcut keypresses on the reset path. Trace label `refresh_focused_viewport source=unanchored_view_reset` names the skip.

`apply_layout_shortcut` lives in `crates/ui/src/window_pane_layout_ops.rs` (split off `window_panes.rs` to keep that file under the conventions cap). It explicitly calls `refresh_focused_viewport_unanchored` and seeds `view.scroll_y_dip` from a cheap caret source-line approximation so the caret lands near the top of the new pane without paying the walker.

## Projection-worker early dispatch on focus/adopt/reopen/layout

`Window::try_dispatch_projection_worker_early(reason)` is hooked from every non-edit transition that changes which buffer/geometry the next paint will render: `switch_focus`, `adopt_buffer_as_new_tab`, `reopen_closed_tab`, and `apply_layout_shortcut`. Submitting at the moment of the geometry change gives the projection worker ~16+ ms head start before the next `WM_PAINT`. The dispatch is fire-and-forget and short-circuits cheaply when the worker hasn't been spawned yet (first paint). Trace event: `event:projection_worker_early_dispatch reason=switch_focus|adopt_buffer_as_new_tab|reopen_closed_tab|apply_layout_shortcut|… submitted=… plan=… stamp_rev=…`.

## Configuration
- `window.restore_to_virtual_desktops` — opt-out switch (defaults on).
- `ui.tab_close_button` — `"hover"` | `"always"` | `"never"`.
- `ui.show_status_bar` — already covered in rendering.

## Key files
- pane tree types: `crates/ui/src/pane_tree.rs`
- pane tree codec: `crates/ui/src/pane_tree_codec.rs`
- pane-tree live-buffer repair: `crates/ui/src/window_buffer_tab_repair.rs`
- per-pane state: `crates/ui/src/pane_state.rs`
- pane layout commands: `crates/ui/src/pane_layout.rs`
- pane mouse/shortcuts: `crates/ui/src/pane_shortcuts.rs`
- Window aggregate: `crates/ui/src/window.rs`
- pane manipulation: `crates/ui/src/window_panes.rs`
- right-edge chrome state + hit-test: `crates/ui/src/window_right_edge_chrome.rs`
- chrome context menus: `crates/ui/src/window_context_menu.rs`
- tab close + reopen-closed: `crates/ui/src/window_panes/close_reopen.rs`
- parent-split inspection for reopen-via-resplit: `crates/ui/src/pane_layout/parent_split.rs`
- smart-reopen handler (global stack + delegation): `crates/app/src/registry_closed_history.rs`
- global closed-history SQLite table: `crates/persist/src/closed_history.rs`
- pane/tab motion: `crates/ui/src/chrome_motion.rs`, `crates/ui/src/motion.rs`
- multi-window persistence: `crates/ui/src/window_placement_persistence.rs`
- virtual desktop COM wrapper: `crates/win/src/virtual_desktop.rs`
- tab/pane chrome paint: `crates/render/src/pane_chrome.rs`
- tab-drag drop-resolver + cross-window broadcast: `crates/ui/src/window_tab_drag.rs`
- tab-drag screen-space tab replica: `crates/ui/src/window_tab_drag_ghost.rs`
- tab-drag overlay payload builder: `crates/ui/src/window_tab_drag_overlay.rs`
- tab-drag painter (insertion bar, source fade, pane-body highlight): `crates/render/src/tab_drag_paint.rs`

## Relates to
- [Buffer](buffer.md) — each tab references one `BufferId`; multiple tabs can share a buffer.
- [Rendering](rendering.md) — every visible `Group` leaf renders chrome + active tab body; non-focused panes paint through `pane_body::paint_all_pane_bodies`.
- [Motion](../motion.md) — pane focus, tab activation, reduced-motion, and stagger contract.
- [Concurrency](../concurrency.md) — every window owns its UI thread + swap chain; nothing UI-shaped crosses windows.
- [Persistence](persistence.md) — `windows` / `panes` / `tabs` / `view_states` rows back the restore protocol.
