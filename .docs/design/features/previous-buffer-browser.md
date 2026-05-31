# Previous-buffer browser (δ.4)

A palette-mode overlay listing every buffer that has ever been persisted to
the SQLite DB, so users can re-open closed buffers without resorting to
direct SQLite queries.

> **See also: [Buffer-history tab](buffer-history-tab.md).** That surface is
> the visual / temporal complement to this overlay — the overlay is best
> when you know the title, the history tab is best when you remember when
> you last edited it. Both surfaces share the same `BufferListFilter`
> discriminant and the same recovery helper. Use `Ctrl+Shift+O` for the
> overlay, `Ctrl+Shift+H` for the history tab.

## Why

Closing a tab is **not** a delete. The rope, snapshots, and edit log all
stay in the live DB; only an explicit `trash` action sets `deleted_at`.
Before δ.4 the only way back to a closed buffer was
`tab.reopen_closed` (session-scoped, single-step) or `Ctrl+O` against a
filesystem path. Users with weeks of notes in the DB had no UI surface
for the closed-but-recoverable set.

The browser exposes the full `buffers` table as a fuzzy-filterable list,
with the same look-and-feel as quick-open and the theme picker. Distinct
from any future cross-buffer search index: this is a buffer-identity
list, not a content index, so it sidesteps the FTS5 drift concern that
killed the original quick-open.

## Surfaces

| Concept | Where |
|---|---|
| Persist query | `PersistClient::list_buffer_records(filter)` returns `Vec<BufferRecord>` |
| Filter | `BufferListFilter::{ActiveOnly, All, TrashedOnly}` (default `ActiveOnly`) |
| Overlay state | `crates/ui/src/previous_buffer_browser.rs` (`PreviousBufferBrowser`, `PreviousBufferRow`) |
| Overlay variant | `OverlayKind::PreviousBufferBrowser` |
| Paint layout | `crates/ui/src/overlay_render_pickers.rs::layout_previous_buffer_browser` |
| Window wire-up | `crates/ui/src/window_previous_buffer_browser.rs` |
| Recovery helper | `continuity_persist::recover_buffer` rebuilds a `Buffer` from its latest snapshot + replayed edit log |
| Command id | `view.previous_buffer_browser_show` |
| Default chord | `Ctrl+Shift+O` (parallel to `Ctrl+O` for the filesystem dialog) |

## UX

- **Open:** `Ctrl+Shift+O` queries persist for `ActiveOnly` rows, sorted
  by `last_touched DESC`. The overlay opens with the focus on the filter
  input.
- **Filter:** Typed characters fuzzy-match against the derived title
  (first non-empty trimmed line of the latest snapshot, leading
  markdown heading markers stripped, clipped to 48 chars). Empty query
  lists every row in original order.
- **Step:** Up/Down (or PageUp/PageDown) walks the selection.
- **Cycle filter:** `Ctrl+T` cycles `Active → All → Trash → Active`,
  re-queries persist, and refreshes the row list. Footer shows the
  current discriminant.
- **Open buffer:** `Enter` adopts the highlighted buffer as a new tab in
  the focused pane. If the buffer isn't already in editor state, the
  Window recovers it from persist (latest snapshot + checksum-validated
  replay of the trailing edit log) before adoption.
- **Open timeline:** `Ctrl+R` while a row is highlighted recovers (if
  needed), adopts the buffer as a new tab, and opens the time-machine
  slider against it — the same slider as the I1 `Ctrl+K Ctrl+B` chord,
  driven against the just-recovered buffer.
- **Dismiss:** `Esc` returns to the editor unchanged.

## Footer hints

The footer line carries the current filter discriminant, the visible /
total counts, and chord hints:

```
Active 23 of 23  ·  Ctrl+T cycle filter  ·  Ctrl+R timeline
```

## Recovery semantics

`continuity_persist::recover_buffer` is the canonical helper for
rebuilding a `Buffer` from persist. Both the previous-buffer browser
(this feature) and the app-side startup recovery path use it. Replay
halts at the first checksum mismatch per spec §4 — partial recovery is
still surfaced rather than dropping the buffer entirely.

## Trash convention

Closed tabs ≠ trashed buffers. The browser surfaces them as separate
states:

- `ActiveOnly` (default): rows with `deleted_at IS NULL`. The common
  case — "I closed this tab and now I want it back".
- `TrashedOnly`: rows with `deleted_at IS NOT NULL`. The recovery path
  for intentional deletes; titles are prefixed with `[trash]`.
- `All`: union, for the rare cross-state browse.

## Stage C — backup snapshot browser (deferred)

The roadmap entry also calls for a secondary overlay that mounts a hot
backup file (`%LOCALAPPDATA%\continuity\backups\<ts>.db`) as a read-only
`rusqlite::Connection` and surfaces its `buffers` through the same
browser logic. Out of scope for the δ.4 first cut; the listing code in
`PersistClient::list_buffer_records` is `&self` so the backup path can
reuse it once a secondary-connection abstraction lands.

## Testing

- Unit tests in `previous_buffer_browser.rs` cover filter logic, cursor
  stepping, humanized-age formatting, and filter cycling.
- Persist tests in `buffer_listing.rs` cover the SQL query under each
  filter discriminant, sort order, and title derivation.
- The integration test in
  `crates/ui/tests/previous_buffer_browser_integration.rs` stands up a
  real `PersistHandle`, persists buffers, queries records, and exercises
  the persist↔overlay handoff (filter cycle, fuzzy filter, confirm
  contract, recovery round-trip) without spinning a Win32 window.
