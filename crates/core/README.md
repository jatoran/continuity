# core

The singleton editor state machine. Single owner of every `Buffer`'s
mutable state; receives `Command` messages and broadcasts `EditEvent`s.

Layer: middle. Depends on `buffer`, `persist`, `text`. Single-writer
rule: nothing else mutates buffers.

The core thread is also the only writer of each buffer's running
checksum. Persisted edit rows read `Buffer::running_checksum()` on the
hot path and run interval / snapshot-boundary verification before
recording or snapshotting a potentially drifted value.

`EditorHandle::apply_edit_group` accepts preplanned `EditOp`s plus the
post-edit selection set and applies them as one undo group on the core
thread. UI uses this for replace-all so large find/replace operations avoid
one round-trip per match while preserving the single-writer rule.
