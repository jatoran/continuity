# Data model

## Scope
- In: SQLite schema, persisted entity shapes, in-memory id newtypes.
- Out: write protocol details (see `features/persistence.md`), per-feature serialization (see each feature doc).

## Vocabulary
- **BufferId**: `Uuid` v7 (time-sortable) — `buffer/src/id.rs`.
- **Revision**: monotonically-increasing `u64` per buffer — bumped on every `Buffer::apply`.
- **UndoGroupId**: `u64` per buffer — groups one logical user action into one undo step.
- **EditOp**: insert / delete / replace at a `Range` — the only atomic mutation primitive.
- **FileAssociation.hash**: raw on-disk byte fingerprint for watcher / self-write detection.
- **FileAssociation.content_hash**: decoded rope-text fingerprint for dirty-tab decisions.
- **SourceByte / DisplayByte / DisplayUtf16**: typed newtypes that prevent mixing coordinate spaces.

## In-memory ID newtypes

All ids are newtypes. None are interchangeable with raw `u64` / `Uuid` at API surfaces.

| Type | Underlying | Crate | Source of truth |
|---|---|---|---|
| `BufferId` | `Uuid` v7 | `buffer` | minted by core on `OpenBuffer` |
| `PaneId` | `Uuid` v7 | `ui::pane_tree` | minted on pane split |
| `WindowId` | `Uuid` v7 | `ui::window_placement_persistence` | minted on window create |
| `TabId` | `Uuid` v7 | `ui::pane_tree` | minted on tab open |
| `Revision` | `u64` | `buffer::revision` | core thread per buffer |
| `UndoGroupId` | `u64` | `buffer::undo` | core thread per buffer |
| `SourceByte` | `u32` | `display_map::id` | absolute byte offset in rope |
| `DisplayByte` | `u32` | `display_map::id` | byte in display string |
| `DisplayUtf16` | `u32` | `display_map::id` | UTF-16 unit in display string (D2D layout coords) |
| `SourceLine` | `u32` | `display_map::id` | source line index |

## SQLite schema

WAL mode, `synchronous=NORMAL` default. Migrations live in `crates/persist/src/schema.rs`.

```sql
buffers (
  id              BLOB PRIMARY KEY,    -- BufferId (Uuid v7)
  title           TEXT,
  file_path       TEXT,                -- nullable; set when file-associated
  file_mtime      INTEGER,
  file_hash       BLOB,                -- raw on-disk bytes
  file_content_hash BLOB,              -- decoded rope text bytes
  created_at      INTEGER NOT NULL,
  last_touched    INTEGER NOT NULL,
  deleted_at      INTEGER,             -- nullable; trash flag
  current_snapshot_id INTEGER,
  current_revision    INTEGER NOT NULL
)

buffer_snapshots (
  id            INTEGER PRIMARY KEY,
  buffer_id     BLOB NOT NULL REFERENCES buffers(id),
  revision      INTEGER NOT NULL,
  created_at    INTEGER NOT NULL,
  content_blob  BLOB NOT NULL,         -- zstd-compressed UTF-8
  content_codec INTEGER NOT NULL,      -- codec version
  byte_len      INTEGER NOT NULL,
  line_count    INTEGER NOT NULL,
  checksum      BLOB NOT NULL,         -- FNV-1a of rope bytes
  label         TEXT                   -- B11: named snapshot label (nullable)
)

buffer_edits (
  buffer_id              BLOB NOT NULL,
  seq                    INTEGER NOT NULL,
  revision               INTEGER NOT NULL,
  ts                     INTEGER NOT NULL,
  op_kind                INTEGER NOT NULL,        -- 0=Insert, 1=Delete, 2=Replace
  range_start            INTEGER NOT NULL,
  range_end              INTEGER NOT NULL,
  removed_text           TEXT,
  inserted_text          TEXT,
  selections_before_json TEXT NOT NULL,
  selections_after_json  TEXT NOT NULL,
  undo_group_id          INTEGER NOT NULL,
  checksum_after         BLOB NOT NULL,           -- FNV-1a of rope after apply
  PRIMARY KEY (buffer_id, seq)
)

undo_groups (
  id              INTEGER PRIMARY KEY,
  buffer_id       BLOB NOT NULL,
  command_name    TEXT NOT NULL,
  ts              INTEGER NOT NULL,
  parent_group_id INTEGER                          -- nullable; supports redo branches
)

windows (
  id                    BLOB PRIMARY KEY,          -- WindowId
  virtual_desktop_guid  BLOB,                      -- IVirtualDesktopManager guid
  monitor_id            INTEGER,
  placement_blob        BLOB,                      -- WINDOWPLACEMENT bytes
  last_seen             INTEGER NOT NULL,
  deleted_at            INTEGER
)

panes (
  id              BLOB PRIMARY KEY,                -- PaneId
  window_id       BLOB NOT NULL,
  parent_pane_id  BLOB,                            -- null = root pane of a Group
  split_axis      INTEGER,                         -- 0=Horizontal, 1=Vertical (null for leaves)
  split_ratio     REAL,                            -- 0.0..1.0
  child_order     INTEGER,
  active_tab_id   BLOB                             -- TabId; null for split nodes
)

tabs (
  id              BLOB PRIMARY KEY,                -- TabId
  pane_id         BLOB NOT NULL,
  buffer_id       BLOB NOT NULL,
  mru_order       INTEGER NOT NULL,
  position_order  INTEGER NOT NULL
)

view_states (
  pane_id            BLOB NOT NULL,
  buffer_id          BLOB NOT NULL,
  scroll_line        INTEGER NOT NULL,
  scroll_subpixel    REAL    NOT NULL,
  selections_json    TEXT    NOT NULL,
  fold_state_json    TEXT    NOT NULL,
  font_size_override REAL,
  soft_wrap          INTEGER NOT NULL,             -- bool
  PRIMARY KEY (pane_id, buffer_id)
)

trash (
  buffer_id   BLOB PRIMARY KEY,
  deleted_at  INTEGER NOT NULL,
  expires_at  INTEGER NOT NULL
)

settings    (key TEXT PRIMARY KEY, value_json TEXT NOT NULL)
keybindings (id INTEGER PK, key_chord TEXT, command TEXT, args_json TEXT, context_predicate TEXT)
themes      (id INTEGER PK, name TEXT UNIQUE, payload_json TEXT)

closed_history (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,    -- newest-first via idx_closed_history_id_desc
  closed_at_ms  INTEGER NOT NULL,
  kind          TEXT NOT NULL,                       -- "window" (reserved: "tab", "pane")
  window_id     BLOB,                                -- nullable; identifies the source window for "window" kind
  payload_json  TEXT NOT NULL                        -- pane-tree blob; same shape as windows.pane_tree_json
)
-- bounded to STACK_CAP = 32 entries via DELETE-on-push inside the persist-thread message
```

Schema `CURRENT_VERSION = 6` (v6 added `buffers.file_content_hash`).

The `fts_buffers` virtual table named in the original spec is **removed** (decisions §K, spec delta §L#17). Cross-buffer FTS5 search has been dropped; `Ctrl+O` opens a native file dialog instead.

## Key type shapes (in-memory)

```rs
struct Position    { line: u32, byte_in_line: u32 }            // text::position
struct Range       { start: Position, end: Position }          // text::range
struct Selection   { anchor: Position, head: Position, kind: SelectionKind }
enum   SelectionKind { Caret, LineWise, BlockWise }
enum   EditOp     { Insert{at, text}, Delete{range}, Replace{range, text} }
struct RopeSnapshot { rope: Arc<Rope>, revision: Revision }
struct EditorSnapshot { rope: RopeSnapshot, selections: Vec<Selection>, file: Option<FileAssociation> }
struct FileAssociation { path: PathBuf, mtime_ms: i64, hash: u64, content_hash: u64 }
```

Cursor positions persist as `(line, byte_in_line)` — survives external edits and is human-readable in the DB.

## Codec versions

`buffer_snapshots.content_codec`:
- `0` — raw UTF-8 (pre-Phase-3 fallback; only old DBs).
- `1` — zstd level 3 compression (current default).

Bump the codec integer (never reuse) when changing the compression scheme. Recovery falls back to the previous codec on decode failure.

## Checksum

`checksum_after` is a per-rope FNV-1a fingerprint computed incrementally as the line-chunk tree updates. On recovery replay the same root is recomputed after each edit row; a mismatch halts replay and surfaces a banner. Whole-rope hashing per edit is **not** acceptable for large buffers; the incremental fingerprint is mandatory.

## Migrations

- Single forward-only migration table (`PRAGMA user_version`).
- Each migration is idempotent (`CREATE TABLE IF NOT EXISTS …`, `ALTER TABLE ADD COLUMN`).
- Schema changes ship paired with the feature that needs them — never a "v1→v2 migrate" job decoupled from a code change.

## Invariants
- `buffer_edits.checksum_after` must verify against the replayed rope at every row. Halt-at-mismatch is mandatory (no silent skip).
- `buffer_snapshots.revision` ≥ the snapshot's first `buffer_edits.revision` after it. Pruning never deletes edits without a covering snapshot.
- Foreign-key style references (`buffer_edits.buffer_id`, `tabs.buffer_id`) are not enforced by `FOREIGN KEY` constraints — the persistence layer joins by id and tolerates missing rows during recovery.

## Constraints + trade-offs
- **`(line, byte_in_line)` positions** ⇒ human-readable in DB, survives external edits ⇒ position resolution costs an extra rope lookup on read.
- **Per-buffer FNV-1a fingerprint** ⇒ O(changed bytes + log n) per edit ⇒ small RAM overhead (line-chunk tree per buffer).
- **No FK constraints** ⇒ recovery is robust to partial corruption ⇒ inconsistencies must be detected by the application, not the DB.
- **Single SQLite file** ⇒ trivial backup, single VACUUM target ⇒ all buffers share write contention (mitigated by WAL).

## Failure modes
- **Snapshot decode fails** ⇒ fall back to previous snapshot; replay from there; on second failure, halt and banner.
- **Edit-row checksum mismatch** ⇒ halt replay at row N, present user-visible banner with timestamp + replayed-revision summary; never silently truncate.
- **Schema migration partial** ⇒ migrations are idempotent — re-running completes them. WAL keeps the previous good state.

## References
- `.docs/development/spec.md` §4 (persistence schema + protocols).
- `crates/persist/src/schema.rs` for the live schema and migrations.
- `.docs/design/features/persistence.md` for write/recovery protocols.
