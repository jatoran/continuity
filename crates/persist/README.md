# persist

SQLite-backed persistence: schema, batched edit log, periodic snapshots,
checksum-verified recovery, and online-backup mirroring.

Layer: foundation+1. Owned by a single persistence thread (single-writer
rule). Depends on `continuity-text` and `continuity-buffer` for the data
shapes it serializes.

## File layout

Top-level: thread + queue (`handle.rs`, `persist_loop.rs`, `message.rs`),
schema + migrations (`schema.rs`), codecs (`codec.rs`, `checksum.rs`),
recovery driver (`recover.rs`), backup scheduler (`backup.rs`), path
resolution (`paths.rs`), error type (`error.rs`), and history / metrics
modules (`metrics.rs`, `timeline.rs`, `window_state.rs`, `file_assoc.rs`,
`buffer_listing.rs`, `handle_buffer_listing.rs`, `handle_timeline.rs`,
`handle_metrics.rs`, `budget.rs`).

Hot-path edit checksums are supplied by `continuity_buffer::Buffer`'s
running FNV-1a state. Persist still owns one-shot snapshot/file checksum
helpers and recovery verification, but the core no longer full-walks
the rope for every appended edit row.

`store.rs` + `store/` carry the `Store` SQLite wrapper. The parent file
defines the shared row structs, the `Store` struct itself, its
constructor / connection accessors, and the helpers each sibling reuses.
Every table family lives in one sibling:

- `store/snapshots.rs` — `buffer_snapshots` writes/reads + the
  descending-revision corruption fallback that recovery relies on.
- `store/edits.rs` — `buffer_edits` append, replay, `next_seq`, and the
  post-snapshot prune.
- `store/buffers.rs` — `buffers` upsert, last-touched bump, and the
  startup most-recent lookup.
- `store/trash.rs` — trash insertion (retention expiry) and the cascade
  purge of expired rows.
- `store/undo_groups.rs` — `undo_groups` insert + ordered reload.
- `store/backup.rs` — SQL-level online-backup driver (the cadence
  scheduler lives in top-level `backup.rs`).

`paths.rs` also exposes `tutorial_seen_path()` — the sentinel file
(`.tutorial_seen` next to `continuity.db`) that gates the first-launch
tutorial open. Existence is the only signal; deletion re-arms first-
launch behaviour. See `.docs/design/features/tutorial.md`.
