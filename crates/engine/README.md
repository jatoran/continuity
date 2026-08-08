# continuity-engine

Synchronous, storage-neutral owner of editor buffers, selections, revisions,
undo state, edit planning, and revisioned change batches. The caller selects
the owning thread. This crate performs no filesystem, database, window,
renderer, worker-pool, or process-global initialization.

The public synchronous facade returns ordered `ChangeBatch` values for host
persistence and revisioned events for later draining. Layer: foundation+2.
Runtime dependencies are `continuity-buffer`, `continuity-text`, `ropey`,
`ahash`, and `thiserror`. Native Windows core owns one `Engine`; embedded hosts
can own one directly.
