# Embeddable Windows editor control

Milestone 6 exposes the native editor surface as a Rust-owned `WS_CHILD`
control. It is a Windows rendering/input adapter over the shared synchronous
engine and host contracts; it is not a second editor implementation and it
does not inherit desktop persistence or application lifecycle.

## Ownership boundary

`continuity_ui::EditorControl` belongs to the thread that constructs it and is
deliberately neither `Send` nor `Sync`. The embedding host owns:

- the parent HWND and its `GetMessage` / `TranslateMessage` /
  `DispatchMessage` loop;
- parent layout, control visibility/enabled state, and application shutdown;
- persistence, durability acknowledgement, files, menus, and host commands;
- draining the bounded `ControlEventSink` receiver.

The control owns its child HWND, one `HostRuntime`, one displayed `BufferId`,
and one `EditorSurface`. Dropping or explicitly destroying the control destroys
only that child. It never posts `WM_QUIT`, saves placement, registers an
application window, flushes desktop services, or creates a database.

The native desktop application remains unchanged as a durable host. Its core
actor owns the same `continuity_engine::Engine` behind the SQLite persistence
bridge, while its `Window` owns the same `EditorSurface` type used by the child
control.

## Rust construction API

`EditorControl::new(parent, bounds, runtime, options, event_sink)` requires a
valid parent HWND and creates a clipped `WS_CHILD | WS_TABSTOP` surface.
`ControlRuntime` supports three ownership modes:

- `Ephemeral { initial_text }` creates a fresh in-memory `HostRuntime`;
- `HostRuntime { runtime, buffer_id }` adopts an open host runtime;
- `Engine { engine, buffer_id }` wraps a prepared storage-neutral engine.

The handle exposes `hwnd`, `buffer_id`, `revision`, `text`, and immutable
`snapshot` queries; normalized `dispatch`; portable-or-host
`dispatch_command`; host-mediated `provide_clipboard_text`; parent layout via
`set_bounds`; focus, visibility, enabled state; and idempotent destruction.

This is the supported Rust control boundary. A C ABI is intentionally deferred
until the later distribution milestone freezes allocator, callback, panic, and
version-negotiation contracts.

## Events and persistence

Every normalized intent runs synchronously through `HostRuntime::dispatch`.
The resulting ordered `HostEventBatch` is delivered to a bounded
`ControlEventSink` only after the mutable engine call has returned. Delivery is
lossless and applies channel backpressure instead of dropping change batches.
Hosts should drain on another thread or provision capacity for their UI-pump
cadence; a disconnected receiver is an explicit control error.

`HostEvent::Change` is the host-managed persistence boundary. Returning from a
control mutation means only that the in-memory engine accepted it. An embedding
host must not claim durability until its own adapter has durably acknowledged
every ordered batch through the reported revision. Ephemeral mode performs no
filesystem or database I/O.

## Native adapter behavior

The child uses the existing DirectWrite/D2D/DXGI `Renderer`, bounded
`LayoutCache`, `FrameDisplay`, and `EditorSurface::projection` retained frame.
The retained projection also supplies source/display mapping for pointer
selection, preventing paint and hit testing from inventing different content.

The child wndproc owns only surface messages:

- UTF-16 `WM_CHAR`, editing/navigation keys, undo/redo, selection, and portable
  command dispatch;
- normalized pointer selection and capture, wheel scrolling, focus publication,
  and `WS_TABSTOP` behavior;
- `WM_SIZE` renderer resize and viewport events;
- `WM_DPICHANGED_AFTERPARENT` font/layout invalidation and swap-chain rebind;
- native Unicode clipboard or host-mediated clipboard requests;
- IME start/update/commit/cancel plus candidate-window positioning;
- context-menu and dropped-file host requests;
- the shared UI Automation Document + Text/Text2 provider, including selection
  mutations marshalled to the control's owner thread.

`TabBehavior::InsertIndent` requests `DLGC_WANTTAB` and inserts a tab through
the engine. `TabBehavior::TraverseHost` leaves Tab in the parent dialog chain
and moves to the next/previous sibling tab stop. Parent resizing is explicit
through `set_bounds`; child clipping is enforced by `WS_CLIPSIBLINGS` and
`WS_CLIPCHILDREN`; per-monitor-v2 DPI inheritance is handled after the parent.

## Host harness and budgets

`continuity_test_support::EditorControlHarness` is a deliberately small
non-Continuity host. It creates a plain top-level parent, owns the ordinary
Win32 pump, and embeds independent controls without `ui::Window`,
`EditorHandle`, SQLite, placement, registry, or app services.

`crates/ui/tests/editor_control_host.rs` covers typing, selection, scrolling,
resize, DPI inheritance, IME lifecycle, clipboard mediation, UI Automation,
multiple controls, host-owned commands, and independent destroy/recreate. It
also proves prepared engine and runtime construction.

The child performance budget remains the native <=8 ms p99
keypress-to-present contract. The 2026-07-17 serial measurement produced
2.538 ms p99 on the development host. The dedicated resource gate budgets
private commit at <=48 MiB per control and idle process CPU at <=5%; its
four-control run measured 32,922,624 bytes per control and 0.00% idle CPU.
Both gates run in release mode through `cargo xtask bench-fast`; run only the
child-control target with:

```powershell
cargo test --release -p continuity-ui --test editor_control_host -- --ignored --test-threads=1 --nocapture
```

The perf tests are ignored in the parallel workspace lane because latency and
process-wide counters are meaningful only in a serial dedicated process.
