# host

Platform-neutral editor intents, typed engine operations, and revisioned host
event batches. `HostRuntime` is an optional synchronous ephemeral composition;
native Windows keeps its existing actor and SQLite bridge while consuming the
same `EditorOperation` contract.

`editor_operation_for_command` resolves context-free Continuity command IDs to
typed operations here so WASM, native controls, and desktop dispatch share one
storage-free command boundary. File, pane, tab, window, settings, and other
host-owned commands deliberately do not resolve. Word and line selection
commands apply `continuity-text` boundaries to every active selection head and
return a selection-only host event batch without advancing the text revision.

The embeddable Windows `EditorControl` owns a `HostRuntime` directly. Runtime
construction can be ephemeral, can adopt a host-prepared runtime, or can wrap
a prepared `Engine`; immutable snapshots expose text, selections, revision,
and read-only state without introducing storage.

`PointerIntent` carries surface-DIP coordinates, lifecycle phase, active and
held buttons, click count, and normalized modifiers. `HostRequest` mediates
plain-text clipboard reads/writes, context menus, link activation, and dropped
files. The
native adapter translates Win32 messages and performs OS I/O outside this
crate; embedders implement the same requests with their platform services.

Layer: portable glue. Depends on `engine`, `buffer`, and `text`. It creates no
threads, windows, files, databases, or callbacks.
