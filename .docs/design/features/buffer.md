# Buffer

The per-document aggregate: rope + revision + selections + undo tree + optional
file association. Only the caller-selected engine owner mutates a `Buffer`;
every other thread sees an immutable `RopeSnapshot = Arc<Rope> + Revision`.

## What it is
- The per-document aggregate lives inside `Engine`; Windows selects the core
  actor as owner, while direct embedders select their own thread.

## Key concepts
- **`BufferId`** — `Uuid` v7 (time-sortable); minted by the engine id source or supplied by a loading host. Never reused.
- **`Revision(u64)`** — monotonic, bumped on every `Buffer::apply`. Stamps every snapshot and worker result.
- **`Selection { anchor, head, kind }`** — `Caret` / `LineWise` / `BlockWise`. Multi-cursor is the general case; single is `selections.len() == 1`.
- **`EditOp`** — only atomic mutation: `Insert { at, text } | Delete { range } | Replace { range, text }`. Resists growth (no `MoveLineUp` variant; that's a sequence under one `UndoGroupId`).
- **`UndoGroupId(u64)`** — groups one logical user action across multiple `EditOp`s. One per `apply_selection_edit` call.
- **`RopeSnapshot`** — `Arc<Rope>` + `Revision`. The only thing that crosses thread boundaries.
- **`FileAssociation`** — optional filesystem link. Carries raw-byte hash for watcher detection and decoded-content hash for dirty-tab decisions.

## Data model

```rs
struct Buffer {
    id:        BufferId,
    rope:      Rope,
    revision:  Revision,
    selections: Vec<Selection>,
    undo:      UndoTree,
    file:      Option<FileAssociation>,
}
```

Positions are `(line, byte_in_line)` — byte-based at the storage layer, line-and-byte at the API layer, grapheme-cluster-based at cursor-movement layer. Persisted positions use the line+byte form so external rope mutations don't invalidate them.

## Operations
- `Buffer::apply(op) -> Revision`: mutates the rope, bumps revision, auto-transforms existing selections through the op (`SelectionTransform::from_op`). This auto-transform is overridden by `apply_planner_group` when the planner supplies explicit `selections_after`.
- `Buffer::set_selections(Vec<Selection>)`: replaces the selection set. Empty defaults to `[Selection::caret_at(Position::ZERO)]` and out-of-range positions clamp to the nearest valid rope position — the buffer always has at least one valid caret.
- `Buffer::snapshot() -> RopeSnapshot`: `Arc<Rope>` clone + revision. Cheap.
- `Buffer::capture_removed_text(&op)`: pre-computes the inverse text for undo records.

### Undo tree
- `UndoTree` is a tree, not a stack. Redo branches survive new edits — see `crates/buffer/src/undo.rs`.
- Each `EditRecord` carries `op`, `inverse_op`, `revision_before`, `revision_after`, `selections_before`, `selections_after`.
- Groups are minted by `continuity-engine`'s undo manager; consecutive keystrokes within a coalesce window merge into one group.

### Undo/redo record delta history (invariant)
- **Invariant: every revision-advancing path records rope delta history.**
  Engine forward edits and undo/redo capture each op's point-augmented delta
  against the pre-apply rope and append it to the same bounded history.
- **Failure mode (Ctrl+Z crash, fixed 2026-06-16):** an undo that advances the rope revision but records no delta makes `rope_deltas_since(old_rev)` return `(empty, covered=true)` — a false "nothing changed". The UI projection classifier then reuses the pre-undo display map against the post-undo rope and slices an out-of-bounds byte range on paint. The fix is structural: the invariant above; the projection classifier carries a defense-in-depth backstop (see [Display map](display-map.md)).

## API surface
- Public crate API: `Buffer::{empty, from_text, id, rope, revision, selections, set_selections, apply, capture_removed_text, undo_tree, undo_tree_mut, file_association, set_file_association, snapshot}`.
- Public mutation goes through synchronous `continuity_engine::Engine`; the
  Windows UI reaches that facade through `core::EditorHandle`. There is no
  `Mutex<Buffer>`.

## Configuration
- None at the buffer level; settings (caret style, indent unit, etc.) live in `config::Settings` and are read by callers.

## Key files
- aggregate: `crates/buffer/src/buffer.rs`
- revisions: `crates/buffer/src/revision.rs`
- undo tree: `crates/buffer/src/undo.rs`
- snapshot facade: `crates/buffer/src/snapshot.rs`
- selection transform on apply: `crates/buffer/src/buffer.rs::SelectionTransform`
- id newtypes: `crates/buffer/src/id.rs`

## Relates to
- [Selections + edits](selection-edits.md) — `SelectionEdit` planner sits on top of `Buffer::apply`.
- [Persistence](persistence.md) — every `Buffer::apply` produces an `EditRecord` for the persist thread.
- [Decoration](decoration.md) — workers consume `RopeSnapshot` and stamp results with `Revision`.
- [Display map](display-map.md) — source-byte coordinates are buffer-owned; display projection is derived.
