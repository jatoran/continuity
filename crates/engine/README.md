# continuity-engine

`continuity-engine` is the editor engine from Continuity, a Markdown notes editor for Windows, as a plain Rust library.
It holds each document's text, selections, undo history and revision number, and applies edits and editing commands to them, including the Markdown ones such as list continuation and bold toggles.
It never touches the filesystem or a database, creates no windows, threads or renderer, and sets up nothing process-global, so your program calls it from whichever thread owns it and decides for itself how to save.

Every mutation returns ordered `ChangeBatch` values for the host to persist, and revisioned events wait for the host to drain them later.
The native Windows app's core thread owns one `Engine`; an embedding host can own one directly.

Layer: foundation+2.
Runtime dependencies are `continuity-buffer`, `continuity-text`, `ropey`, `ahash`, and `thiserror`.
