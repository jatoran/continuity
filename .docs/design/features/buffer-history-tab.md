# Buffer-history tab

A non-buffer tab that renders a **horizontal swimlane chart** of every
persisted buffer: one row per buffer, snapshot dots laid out along a
shared time axis. Complement (not replacement) of the
[previous-buffer browser](previous-buffer-browser.md) overlay.

## Why

The previous-buffer browser overlay is keyboard-fast for **lookup by
title**: "I know what I called it, just open it." That's the wrong
shape when the affordance the user wants is **temporal recall**:
"I edited it last night, I can't quite remember the title." A
horizontal time axis turns the dropdown's `last_touched DESC` sort
into something the user can scan visually — yesterday is a position,
not a string.

The two surfaces live side-by-side. They serve different jobs:

|  | Previous-buffer browser overlay | Buffer-history tab |
|---|---|---|
| Surface | Palette-mode overlay | Top-level tab |
| Chord | `Ctrl+Shift+O` | `Ctrl+Shift+H` |
| Affordance | Title-fuzzy-filter | Time-axis visual scan |
| Sort | `last_touched DESC` (textual) | `last_touched DESC` (spatial) |
| Persistence | Transient | Round-trips through pane tree |

## Layout

- **Custom renderer.** The tab does not reuse the markdown paint
  pipeline (rope → display map → decorations → layout cache → D2D).
  It has its own Direct2D draw in
  [`render::buffer_history_panel`](../../../crates/render/src/buffer_history_panel.rs).
  Layout is pure (`compute_buffer_history_panel_layout`) so input
  hit-tests share the renderer's coordinate system without owning a
  D2D context.
- **Two-band ruler** (52 DIP) at the top: header chip line
  ("Buffer history · `<filter>`" left, total/today/week/month/older
  bucket counts right) over a tick row whose labels (`14:30` /
  `May 8 14:00` / `May 8` / `May 2026`, granularity scaling with
  viewport span) line up with faint vertical guides extending
  down through every lane.
- **One swimlane per buffer** (56 DIP). Lanes are top-to-bottom,
  sorted `last_touched DESC` (lane 0 = most recently active).
  Title row (history row clipped to 32 chars) plus a subtitle with
  last edit age, edit count, line count, and char count on the left;
  timeline strip vertically centered on the right.
- **Snapshot dots.** Every persisted `buffer_snapshots` row
  projects to one dot on its lane's timeline strip. Dots outside
  the visible viewport are clipped.
- **Preview band** (110 DIP) along the bottom: hovered lane's
  title + first ~6 lines of materialized current content. Falls
  back to selected lane when nothing is hovered; falls back to a
  muted hint when neither is set.
- **Trashed rows.** When the filter is `All` or `TrashedOnly`,
  trashed lanes paint with the muted-foreground brush and a
  `[trash] ` title prefix.
- **Empty state.** When no buffers match the filter, the lane area
  reads `"No buffers in history yet. Notes appear here after they have content or a file path."`.

## Input

| Action | Effect |
|---|---|
| Wheel | Vertical scroll through lane list (3 lanes per notch) |
| Ctrl+Wheel | Zoom time axis about pointer's projected timestamp |
| Shift+Wheel | Horizontal pan (10% of viewport width per notch) |
| Click on lane | Selects + opens that buffer as a new tab |
| Click+drag on empty timeline area | Pan the viewport horizontally |
| Hover | Updates preview band to that lane |
| Arrow ↑↓←→ | Step the selected lane (auto-scrolls to keep visible) |
| PgUp / PgDn | Step ±10 lanes |
| Home / End | Jump to top / bottom |
| Enter | Adopt the selected lane's buffer |
| Esc | Close the history tab |
| `Ctrl+T` (planned) | Cycle filter — wired on state, no chord predicate yet |

After any viewport change (wheel scroll / zoom / pan), the hover
hit-test re-runs against the cursor's current position so the
preview band always reflects the row at the pointer and never
lags behind the layout.

## Single source of truth: replayed content

The lane's **title**, **preview** text, **line count**, and
**char count** are derived
from the same materialized current content via
`Store::load_content_at_revision(buffer_id, Revision(i64::MAX as u64))`,
the same snapshot-plus-edit-replay path the recovery flow uses on
click. Titles use the same first-line display rule as persisted
buffer pickers: trim, strip leading markdown heading markers, and
clip to 48 chars before the UI row applies its 32-char lane cap.
Line counts use the same newline-count-plus-one convention as the
editor rope; char counts count Unicode scalar values.
Earlier drafts pulled title from the latest snapshot's first line
and preview from the snapshot blob; that diverged from
the opened buffer whenever edits after the snapshot changed the
content (most commonly: typed → snapshot fired → cleared →
title still showed old line, preview ran replay and produced
blank). Same source means title / preview / counts / opened-tab
can't disagree.

**Sentinel caveat.** The replay uses `Revision(i64::MAX as u64)`
rather than `Revision(u64::MAX)`: the underlying SQL stores
revisions as `INTEGER` and binds via `revision.get() as i64`, so
`u64::MAX` wraps to `-1` and the `WHERE revision <= ?` predicate
matches zero rows. `i64::MAX as u64` is still well past any
realistic revision count and casts back to `i64::MAX` cleanly.

## Synthetic render buffer

Each history tab is backed by a synthetic empty `Buffer`
(`Buffer::synthetic_read_only("")`) allocated lazily on first
open and cached on the window:
`view_options.buffer_history_render_buffer_id: Option<BufferId>`.
The tab's `Tab.buffer_id` points at that synthetic buffer (NOT
[`BufferId::nil`]); the kind discriminant
[`TabKind::BufferHistory`] is what differentiates the surface.
This mirrors the tutorial-tab + metrics-buffer pattern and lets
the regular paint pipeline (`Renderer::draw_buffer_no_present`)
run with an empty rope behind the panel — so the tab strip,
status bar, and pane border all paint normally and the history
panel overlays only the focused pane's body rect.

`Tab.buffer_id_opt()` returns `None` for `TabKind::BufferHistory`
regardless of the underlying synthetic id, so file save /
decoration scheduling / autosave paths that branch on
`buffer_id_opt` still skip the surface cleanly.

## Data flow

1. User invokes `view.buffer_history` (default `Ctrl+Shift+H`,
   also reachable from the command palette as a palette-safe
   entry).
2. `Window::show_buffer_history_tab_impl` ensures the synthetic
   render buffer exists, opens (or focuses) the single history
   tab in the focused pane, allocates / reuses the per-tab
   [`BufferHistoryTab`](../../../crates/ui/src/buffer_history_tab.rs) state.
3. `Window::refresh_buffer_history_tab` calls
   `PersistClient::list_buffer_history_timeline(filter)` which
   issues `PersistMessage::ListBufferHistoryTimeline` and blocks
   on the reply channel.
4. Persist thread runs `Store::load_buffer_history_timeline`:
   - `list_buffer_records(filter)` for buffer summaries
     (filtered to exclude orphans — see below),
   - bulk `SELECT buffer_id, created_at FROM buffer_snapshots`
     grouped by id for per-lane snapshot timestamps,
   - `load_content_at_revision(id, Revision(i64::MAX as u64))`
     per record to materialize current content for the
     `(title, preview)` pair via `derive_title_and_preview` and
     the row's line / char counts via `derive_content_counts`.
5. Paint runs the regular pipeline with the synthetic empty
   rope (chrome + empty body), then `paint_buffer_history_panel`
   overlays the focused pane's body rect.

## Orphan filter + startup sweep

Empty unedited buffers (typically the residue of `tab.new`
sessions the user closed without typing) are filtered out at
query time and hard-deleted at startup:

- **Filter** (`Store::list_buffer_records`): a buffer must have
  edits OR a non-zero-byte snapshot OR a file association.
  Zero-byte baseline snapshots (written automatically on adopt)
  do not qualify as content.
- **Sweep** (`Store::purge_orphan_buffers`, called once from
  `PersistHandle::spawn`): hard-deletes orphan `buffers` rows
  matching the same predicate, then cascades to dangling
  `buffer_snapshots` / `buffer_edits` rows. Trashed buffers
  (`deleted_at IS NOT NULL`) are never touched.

The combined effect: the history view shows real buffers only,
the DB shrinks on each startup until orphans stop accumulating.

## Persistence (of the tab itself)

The history tab round-trips through the pane-tree JSON blob like
any other tab. `WireTab` carries a `#[serde(default)] kind`
field that defaults to `TabKind::Buffer` for backward compat
with blobs written before the discriminant existed. On restore,
history tabs reappear with a stale `buffer_id` pointing at a
synthetic buffer that no longer exists; the on-paint path
detects this (`editor.snapshot(buffer_id).is_none()`) and
lazy-allocates a fresh synthetic render buffer, rewriting
`tab.buffer_id` to match. `BufferHistoryTab` state is fresh on
restore — persist is re-queried on first focus.

`buffer_ids_in_json` (used at app startup to pre-load every
tab's buffer) **skips** non-buffer tab kinds so the startup
loader never attempts to fetch the stale synthetic id.

## Surfaces

| Concept | Where |
|---|---|
| Persist query | [`Store::load_buffer_history_timeline`](../../../crates/persist/src/buffer_history.rs) |
| Orphan filter | [`Store::list_buffer_records`](../../../crates/persist/src/buffer_listing.rs) |
| Startup sweep | [`Store::purge_orphan_buffers`](../../../crates/persist/src/store/buffers.rs) |
| Title+preview derivation | `derive_title_and_preview` in [`buffer_history.rs`](../../../crates/persist/src/buffer_history.rs) |
| Persist client | [`PersistClient::list_buffer_history_timeline`](../../../crates/persist/src/handle_buffer_history.rs) |
| Lane payload | [`BufferHistoryLane`](../../../crates/persist/src/buffer_history.rs) |
| Tab kind discriminant | [`TabKind`](../../../crates/ui/src/pane_tree_kind.rs) |
| Per-tab state | [`BufferHistoryTab`](../../../crates/ui/src/buffer_history_tab.rs) |
| Window wire-up | [`window_buffer_history_tab.rs`](../../../crates/ui/src/window_buffer_history_tab.rs) |
| Paint dispatch | [`window_paint.rs`](../../../crates/ui/src/window_paint.rs) `history_overlay` branch |
| Mouse / wheel routing | [`window_mouse.rs`](../../../crates/ui/src/window_mouse.rs) + [`window_runtime.rs`](../../../crates/ui/src/window_runtime.rs) `on_mouse_wheel` |
| Keyboard | [`window_commanding.rs`](../../../crates/ui/src/window_commanding.rs) `on_keydown` |
| Custom paint | [`render::buffer_history_panel`](../../../crates/render/src/buffer_history_panel.rs) |
| Time-axis ticks | `compute_time_axis_ticks` in [`buffer_history_panel.rs`](../../../crates/render/src/buffer_history_panel.rs) |
| Command id | `view.buffer_history` ([`buffer_history.rs`](../../../crates/command/src/buffer_history.rs)) |
| Default chord | `Ctrl+Shift+H` |

## Testing

- Persist unit tests in
  [`crates/persist/src/buffer_history.rs`](../../../crates/persist/src/buffer_history.rs)
  cover SQL under each filter discriminant, lane ordering,
  and the orphan-filter edge case.
- Orphan-sweep tests in
  [`crates/persist/src/store/buffers.rs`](../../../crates/persist/src/store/buffers.rs)
  cover the zero-byte-snapshot case and the trashed-rows
  exemption.
- Handle round-trip tests in
  [`crates/persist/src/handle_buffer_history.rs`](../../../crates/persist/src/handle_buffer_history.rs)
  validate the `PersistMessage` plumbing.
- UI state tests in
  [`crates/ui/src/buffer_history_tab.rs`](../../../crates/ui/src/buffer_history_tab.rs)
  cover viewport math (zoom-about preserves pivot fraction,
  pan preserves width, min-width clamp, lane stepping clamps,
  filter cycle round-trips).
- Renderer tests in
  [`crates/render/src/buffer_history_panel.rs`](../../../crates/render/src/buffer_history_panel.rs)
  cover layout (one row per draw row, clip at panel bottom),
  projection (`fraction_for` round-trip), hit-test, and the
  ruler-bucket hint.
- Integration test in
  [`crates/ui/tests/buffer_history_tab_integration.rs`](../../../crates/ui/tests/buffer_history_tab_integration.rs)
  drives a `BufferHistoryTab` through filter / step / confirm
  against a real `PersistHandle` — no Win32 surface.

## What's out of scope

- **Branch / fork semantics.** Buffers are flat — no
  `parent_buffer_id`, no fork command. The visualization is
  purely temporal.
- **`Ctrl+T` filter cycle binding.** The state method exists
  (`BufferHistoryTab::cycle_filter`) and the window wiring is
  present, but no keymap chord is currently bound when the
  history tab is focused. Add a `tab.kind == 'buffer_history'`
  context predicate to surface it.
- **Click-on-dot to open at that revision.** The snapshot dots
  are static markers; cross-linking with the time-machine
  slider so a click on a dot opens at that revision is a
  future pass.
