# Persistence

SQLite-backed durable storage with WAL journaling, FNV-1a checksummed edit log, periodic zstd-compressed snapshots, and hot mirror backups. Every keystroke reaches disk within 400 ms p99; recovery is structural (replay the edit log from the latest valid snapshot, halt at the first checksum mismatch), never best-effort.

## What it is
- SQLite-backed durable storage with batched edits, periodic snapshots, hot mirror backups, and a halt-at-mismatch recovery protocol. Every keystroke reaches disk within `≤400 ms p99`; recovery never silently truncates.

## Key concepts
- **`EditRecord`** — one buffer mutation row: `(buffer_id, seq, revision, ts, op_kind, range, removed_text, inserted_text, selections_before/after_json, undo_group_id, checksum_after)`.
- **Snapshot** — zstd-compressed UTF-8 rope blob + FNV-1a checksum + revision stamp.
- **Snapshot policy** — fires on 500 edits OR 256 KiB cumulative changed bytes OR 60 s of activity OR explicit close.
- **`PersistClient`** — clonable handle (a `crossbeam Sender`) into the persist thread.
- **`PersistHandle`** — the owning thread + `JoinHandle`; held by `app::registry`.
- **Hot backup** — `rusqlite::backup` every 15 min into `%LOCALAPPDATA%\continuity\backups\session-N.db`.

## Data model
See [`data_model.md`](../data_model.md) for the schema. Tables: `buffers`, `buffer_snapshots`, `buffer_edits`, `undo_groups`, `windows`, `panes`, `tabs`, `view_states`, `trash`, `settings`, `keybindings`, `themes`, `closed_history`.

`fts_buffers` is removed (Phase G6 / spec delta §L#17). Schema
`CURRENT_VERSION = 6`; v5 added `closed_history`, v6 added
`buffers.file_content_hash` for decoded-text dirty checks.

## Operations

### Write protocol (per accepted edit, on core thread)
1. `Buffer::apply(op)` mutates the rope, bumps revision, auto-transforms selections, and updates the buffer-owned running FNV-1a checksum.
2. Core pushes an `EditRecord` onto the persist channel with `checksum_after = Buffer::running_checksum()`; every `CHECKSUM_VERIFY_INTERVAL` edits, and before snapshot-threshold captures, core verifies the running value against a full rope walk and reseats on drift.
3. Persist thread drains the queue on a 250 ms timer or 64 KiB threshold.
4. Persist begins a transaction, inserts edit rows, commits.
5. If snapshot policy fires, persist serializes the rope (zstd), writes a `buffer_snapshots` row, then prunes covered `buffer_edits` rows.

### File adopt (import) — not an edit
1. File-I/O thread reads + decodes the file, computes raw-byte and decoded-content metadata.
2. File-I/O sends the content to core via `EditorMessage::AdoptBuffer`.
3. Core creates the buffer at an initial revision with the `FileAssociation` attached and stamps `content_hash` from the exact imported text.
4. Persist writes the buffer row + initial snapshot for that revision.
5. The edit log begins only with user edits after the adopted snapshot. Opening a 100 MB file does **not** create a 100 MB `buffer_edits` row.

### Recovery (on startup, per buffer)
1. Load latest `current_snapshot_id`. Decompress, verify checksum. On corruption, fall back to the previous snapshot; recurse if necessary.
2. Replay `buffer_edits` where `revision > snapshot.revision`. Verify `checksum_after` at each row.
3. On any verification mismatch, halt at that row, log it, present a non-blocking banner with timestamp + replayed-revision summary.
4. Restore selections + view states; then `windows` / `panes` / `tabs`.

### Trash
- `buffers.deleted_at` flags soft-delete. `trash` carries an expiry (`expires_at`, default 30 days).
- Recently-closed panel browses `trash`.
- `VACUUM` runs on idle when trash expiries fire.
- Hard-purge command exists for sensitive content.

### Compaction
- Background job every 24 h of cumulative idle: snapshot any buffer with >2000 unsnapshotted edits, prune covered edit rows, `PRAGMA wal_checkpoint(TRUNCATE)`, `PRAGMA optimize`.
- Full `VACUUM` only on explicit user command.

### Smart `tab.reopen_closed` (Ctrl+Shift+T)

The reopen-closed handler picks between two stacks:

1. **Local per-window stack** — `PaneTree::recently_closed` (in
   memory, cap matches the global stack). Closed tabs / panes push
   here.
2. **Global `closed_history` table** — durable across launches and
   across windows. `archive_closed_window` snapshots
   `pane_tree_json` + `window_id` into a `closed_history` row
   *before* tombstoning the source row, so a crash between the two
   leaves the window recoverable via either path. The table is
   bounded at `STACK_CAP = 32` entries (oldest evicted on push,
   inside the same persist-thread message).

The handler compares `local_recently_closed_top_ms` (per-window
`PaneTree::recently_closed[0].closed_at_ms`) against the global
stack's top `closed_at_ms`. The more recent close wins. Ties favor
the global stack (a whole-window close is at least as significant as
a sibling-tab close). When the global stack wins, the entry is
popped and a `SpawnRequest` is dispatched; the local stack is left
alone.

`closed_history` columns: `id INTEGER PRIMARY KEY AUTOINCREMENT`,
`closed_at_ms INTEGER`, `kind TEXT` (`"window"`, plus reserved
`"tab"` / `"pane"`), `window_id BLOB`, `payload_json TEXT` (the
pane-tree blob — same shape as `windows.pane_tree_json`). Newest-first
iteration is `ORDER BY id DESC LIMIT N` via
`idx_closed_history_id_desc`. `ClosedHistoryKind::from_str` returns
`Option`, so unknown TEXT values from a future schema cannot crash a
downgraded reader.

### Session-restore decoder robustness

Strict pane-tree JSON decode failures no longer drop the window
silently. `seed_buffer_for_row` (in
`crates/app/src/main_initial_requests.rs`) never returns `Err`:

1. Strict `buffer_ids_in_json` is attempted first.
2. On failure, `buffer_ids_in_json_lenient` walks the JSON via
   `serde_json::Value` and extracts whatever tab buffer ids are
   well-formed. The window opens with one of those buffers.
3. If even lenient extraction returns empty, fall through to
   `recover_or_open` which seeds the window with the most-recently-
   touched buffer.

In all three cases the window stays on the active list. A sticky
`recovery_notices` entry attaches a `FileBanner` containing
`"decoder error"` to the first paint. The legacy
`WireImageExpand { url: String }` legacy field (renamed to
`source_byte: u64` in commit `a0214a9`) decodes through a hand-rolled
`Deserialize` in `pane_tree_codec/legacy.rs` that accepts both shapes;
the legacy entry decodes with `source_byte = 0`.

### Hot backup
- `rusqlite::backup` reads the live DB through SQLite's online backup API — no file-copy races.
- Default: every 15 minutes if DB changed, last 24 retained, then daily for 30 days.
- "Open backup" command opens the chosen `.db` as a read-only "ghost session" the user can browse and selectively pull buffers from.

## API surface
- UI threads talk to persist via `PersistClient::enqueue_edit_row`, `enqueue_snapshot`, `touch_buffer`, `delete_buffer`, `save_window/pane/tab/view_state`, `settings_get/put`, etc.
- No reads on the hot path — UI re-snapshots from the live `Buffer`, not from disk.
- Recovery is a single `PersistHandle::recover_all(&mut state)` call at startup.
- Edit-row checksum production uses `Buffer::running_checksum()`, `Buffer::verify_running_checksum()`, and `continuity_buffer::update_running_checksum` on the core-owned buffer; persist stores and later verifies the resulting `checksum_after` values.

## δ.3 user-visible surfaces

The principles doc requires that **recovery halt** and **write failure** both surface as user-visible banners — the "saving = export" durability promise is only meaningful if a broken writer is observable. Both flow through the existing `FileBanner` surface; no new banner system.

### Recovery halt banner

`crates/persist/src/recover.rs` exports `RecoveryHalt { buffer_id, halted_at_seq, last_valid_revision, reason: RecoveryHaltReason }` and `rebuild_buffer_with_halt(...) -> (Buffer, Option<RecoveryHalt>)`. `RecoveryHaltReason` is one of:

- `DecodeFailed(message)` — the persisted edit row couldn't be decoded into an `EditOp`.
- `ApplyFailed(message)` — the decoded op couldn't be applied to the rope (revision drift, position out of bounds).
- `ChecksumMismatch { computed, expected }` — the rope's FNV-1a fingerprint after applying the op didn't match `EditRow::checksum_after`. **Spec §4 mandates halt-on-mismatch**; we never propagate past the first divergence.

The buffer returned alongside a halt carries the *last valid* rope — the state that survived the most-recent checksum verification. Subsequent edits start from that point.

`crates/app/src/main.rs::build_initial_requests` collects halts across every recovered buffer and threads them as `Vec<String>` notices on the first `SpawnRequest`. The list reaches `Window::new` via `WindowCommands::initial_banners` and surfaces as a sticky `FileBanner` at startup (first message verbatim; "(+ N more)" appended when multiple buffers halted).

### Write-failure / thread-stopped events

`crates/persist/src/events.rs` introduces:

- `PersistEvent::WriteFailed { kind: PersistOperation, message: String }`
- `PersistEvent::ThreadStopped`

`PersistOperation` names the originating `PersistMessage` variant (`AppendEdit`, `SaveSnapshot`, `UpsertBuffer`, `TouchBuffer`, `PruneCoveredEdits`, `MoveToTrash`, `Backup`, `SetSynchronous`, `WriteUndoGroup`, `SaveWindow`, `SetBufferFile`, `SetSnapshotLabel`, `RecordMetricsDelta`). The `message` is the `thiserror`-formatted error from the failing store call — for SQLite errors that's the SQLite error string (including `ENOSPC` / `SQLITE_FULL` / `SQLITE_PERM` text where applicable).

`persist_loop` emits one event per write failure (alongside the historical `eprintln!` so ops logs keep the same line). On clean shutdown the loop emits a final `ThreadStopped` event *before* the `Sender` drops, so the registry can distinguish a planned exit from a panic-driven channel disconnect.

`PersistHandle::events()` exposes a `Receiver<PersistEvent>` (cloneable; the registry is the sole consumer). The registry's `select!` loop fans events out via `WindowControl::PersistEvent(...)` to every live window; each window's control-poll tick converts the event to a sticky `FileBanner` via `PersistEvent::banner_text()`. On channel disconnect without a preceding `ThreadStopped` (panic case), the registry synthesizes a `ThreadStopped` itself so the banner still fires.

### Files
- `crates/persist/src/recover.rs::{RecoveryHalt, RecoveryHaltReason, rebuild_buffer_with_halt}`
- `crates/persist/src/events.rs::{PersistEvent, PersistOperation}` + `PersistEvent::banner_text`
- `crates/persist/src/persist_loop.rs::report_write_failure`
- `crates/persist/src/handle.rs::PersistHandle::events`
- `crates/app/src/registry.rs::fan_out_persist_event` + disconnect-detection arm
- `crates/ui/src/window_control.rs::WindowControl::PersistEvent`
- `crates/ui/src/window_settings_reload.rs::handle_persist_event`

## Configuration
- `persistence.mode` = `"safe"` (`synchronous=FULL`) | `"normal"` (`synchronous=NORMAL`, default).
- `persistence.snapshot_every_edits` (default 500).
- `persistence.snapshot_every_kib` (default 256).
- `persistence.snapshot_every_seconds` (default 60).
- `backup.cadence_minutes` (default 15).
- `backup.daily_retention` (default 30 days).
- All hot-reloadable.

## Key files
- handle + thread loop: `crates/persist/src/handle.rs`, `crates/persist/src/persist_loop.rs`
- `Store` SQLite wrapper: `crates/persist/src/store.rs` (struct + ctor + shared row types) and its responsibility-scoped siblings under `crates/persist/src/store/` — `snapshots.rs`, `edits.rs`, `buffers.rs`, `trash.rs`, `undo_groups.rs`, `backup.rs`
- schema + migrations: `crates/persist/src/schema.rs`
- file-association storage: `crates/persist/src/file_assoc.rs`
- snapshot codec: `crates/persist/src/codec.rs`
- incremental checksum: `crates/persist/src/checksum.rs`
- paths: `crates/persist/src/paths.rs`
- recovery driver: `crates/persist/src/recover.rs`
- backup scheduler (cadence + retention): `crates/persist/src/backup.rs`
- running edit checksum: `crates/buffer/src/checksum.rs`, `crates/buffer/src/buffer.rs`, `crates/core/src/undo.rs`, `crates/core/src/dispatch.rs`
- closed-history CRUD + stack cap: `crates/persist/src/closed_history.rs`
- smart `tab.reopen_closed`: `crates/app/src/registry_closed_history.rs`
- pane-tree codec legacy / lenient decode: `crates/ui/src/pane_tree_codec/legacy.rs`
- session-restore seed (no-skip + partial-restore banner): `crates/app/src/main_initial_requests.rs`

## Relates to
- [Buffer](buffer.md) — every `Buffer::apply` produces an `EditRecord`.
- [Concurrency](../concurrency.md) — persist queue is byte-accounted with coalesce-on-full backpressure.
- [Settings](settings.md) — `[persistence]` and `[backup]` blocks live here.
- [Trash + retention](file-io.md) — file open/save also goes through the file-I/O thread, not directly through persist.
