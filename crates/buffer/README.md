# buffer

Owns the `Buffer` aggregate: id, rope, revision counter, selections,
undo tree, decoration cache, and per-pane view state.

Layer: foundation+1. Depends on `continuity-text` only. A `Buffer` is mutated
only by its caller-selected `Engine` owner. Native desktop uses the core actor
as that owner.

Buffers can be flagged `synthetic` (never persisted — core skips
`persist.touch_buffer` / `save_snapshot` / `write_edit`) and / or
`read_only` (`apply` short-circuits with `Error::ReadOnly` before
touching the rope). `Buffer::synthetic_read_only(text)` sets both —
used by the tutorial tab and any future static-content surface.

Each buffer also owns a running FNV-1a checksum of its rope bytes.
`Buffer::apply` updates it incrementally per edit; the engine periodically
verifies it with a full walk before returning `checksum_after` values.
