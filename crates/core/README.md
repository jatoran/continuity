# core

Native threaded host for `continuity-engine`. The core actor owns one
`Engine`, receives `EditorMessage`s, broadcasts desktop `EditEvent`s, and
projects engine `ChangeBatch` values into SQLite writes and snapshot policy.

Layer: middle. Depends on `host`, `engine`, `persist`, `buffer`, and `text`. In the
Windows application the core thread is the engine's sole mutable owner;
embedded hosts may own an engine directly on their selected thread.

The actor's selection-edit message carries `host::EditorOperation`, so native
Windows and embedded bindings enter the same typed mutation contract before
native persistence and snapshot policy run.

The engine maintains and periodically verifies each buffer checksum. The
native persistence bridge keeps database sequence numbers and encodes/writes
the batch's per-op checksums without putting SQLite in the engine.

Multi-cursor insertion planners compute post-edit selections through the
entire descending op sequence; per-cursor local positions are never treated
as final coordinates when an earlier cursor can shift later content.

`EditorHandle::apply_edit_group` accepts preplanned `EditOp`s plus the
post-edit selection set and applies them as one engine undo group. UI uses
this for replace-all so large operations avoid one round-trip per match.
