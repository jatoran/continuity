# Architecture

## Scope
- In: runtime topology, thread roles, layer graph, ownership rules.
- Out: per-feature internals (see `features/*`), Win32 message dispatch details (`technical/paint-flow.md`).

## Vocabulary
- **Engine owner**: the caller-selected thread holding `&mut Engine`.
- **Core thread**: native Windows actor and engine owner; adapts messages,
  persistence, and snapshot policy.
- **UI thread**: Win32 message pump + render submission; `DesktopShell` owns it
  for the app, while an embedding host owns it for `EditorControl`.
- **Decoration worker**: pool member that turns `(RopeSnapshot, Revision)` into `Decorations`.
- **Persistence thread**: SQLite-connection owner, drains the bounded edit queue.
- **File-I/O thread**: reads/decodes imported files, writes exports.
- **Snapshot**: an `Arc<Rope>` (cheap clone) plus its `Revision` stamp. Only thing that crosses thread boundaries.

## Process model

Single process, multiple top-level Win32 windows, one shared editor core. Per
window: a `DesktopShell` owns the private message pump and composes `Window`;
`Window` owns the swap chain and desktop pane/tab/session state while its
`EditorSurface` owns editor input/composition, editor-body pointer and
selection-drag state, the focused viewport and scroll/reveal coordination,
caret presentation transients, and projection/cache state.
Its `pointer` module defines platform-independent editor-body interaction
state; `mouse::MouseState` is limited to HWND capture and desktop chrome
drags. The native pointer adapter converts Win32 messages to normalized
surface-DIP intents. The surface also publishes immutable accessibility state;
the Windows UI Automation Text/Text2 provider reads that state and marshals
selection mutations to the UI owner thread.
Cross-window shared: `EditorHandle`, persistence client, decoration pool, theme
set, command registry, settings watcher.

An embedded native host is a separate composition in the same process model.
Its UI thread owns a parent HWND and pump; each `EditorControl` is a `WS_CHILD`
containing a caller-thread-owned `HostRuntime` plus the same `EditorSurface`
used by the desktop. There is no core actor or persist thread unless the host
chooses to build its own adapters. See
[`features/embeddable-windows-control.md`](features/embeddable-windows-control.md).

A WASM host is a third composition. One JavaScript agent owns a synchronous
`RawEditor`/`Engine`; decoration and display projection run inline on that
agent. No native actor, worker pool, filesystem, SQLite thread, HWND, or
renderer enters the WASM closure. `packages/editor` wraps the raw binding with
the stable TypeScript facade; the host owns persistence and presentation.

The cross-platform desktop application composes that WASM host inside Electron.
Its main process is the sole owner of application windows, menus, settings,
files, external-change watchers, updates, and a two-slot host-managed durable
store. An isolated preload exposes named IPC operations. The sandboxed renderer
owns one Web Component and serializes change events until main acknowledges a
synced snapshot. It never imports SQLite or the native Win32 shell; the native
application remains an independent product and persistence adapter. See
[`features/cross-platform-desktop.md`](features/cross-platform-desktop.md).

Continuity Web also takes Electron's single-instance lock before opening the
store. A later launch forwards its document path through Electron's
`second-instance` event and exits, so exactly one main process owns settings,
file watchers, update state, and the durable slots for an application data
root.

**Single instance per data dir.** The process holds a named mutex keyed by the database path (`win::single_instance::SingleInstanceMutex`). A second launch is *not* a second process running the full session — it forwards its command-line file/folder paths to the running instance over a message-only `WM_COPYDATA` hub (`win::single_instance::InstanceHub`, spawned only by the mutex-holding primary) and exits. A bare relaunch enumerates visible same-process windows in z-order and activates the first one that `IVirtualDesktopManager::IsWindowOnCurrentVirtualDesktop` accepts; when none exists (or the COM query is unavailable), it sends a non-restored blank `RegistryEvent::Spawn`, so Win32 creates the HWND on the invoking desktop instead of switching desktops. Only when no live instance is reachable does the launcher run standalone. `--new-instance` (and the `CONTINUITY_E2E_INSERT` test hook) bypass the handoff. Claim/forward logic: `app::single_instance::claim_or_forward`; forwarded **files** route through `RegistryEvent::OpenFileBuffer` (same path as in-process opens — dedup, reveal-existing-tab-or-spawn, and reconcile against current disk bytes; see [file-io](features/file-io.md) §Reconciliation), forwarded **folders** through `RegistryEvent::Spawn`, and a bare relaunch through current-desktop activate-or-spawn.

## Thread map

| Thread | Owns | Reads | Sends |
|---|---|---|---|
| `core` | `Engine`, snapshot trackers, SQLite bridge sequence state | engine state | `EditEvent` broadcast, encoded batch rows to persist |
| `ui-window-N` | `DesktopShell` message pump, `HWND`, `Window`, `EditorSurface` input/pointer/viewport/caret/projection state, swap chain, pane tree | `Arc<RopeSnapshot>`, `Arc<Decorations>`, `Arc<DisplayMap>` | `EditorMessage` to core, paint to D2D |
| `host-ui-N` | host pump/parent HWND; `EditorControl`, `HostRuntime`, `Engine`, `EditorSurface`, child swap chain | host-owned state plus immutable engine snapshots | ordered `HostEventBatch` to bounded host sink, paint to D2D |
| JavaScript agent | npm `Editor`, WASM `Engine`, synchronous decorations/display map | host-provided initial text and selections | returned change/snapshot/projection data to its caller |
| Electron main process | Continuity Web windows, menus, file association/watcher, settings, updater, durable JSON slots | validated renderer IPC and OS file events | durable acknowledgements and host events through isolated preload |
| `decorate-worker-K` (pool) | nothing | `(RopeSnapshot, Revision)` request | `DecorateResult` back to UI |
| `persist` | SQLite conn, edit queue | `EditRecord` from core | nothing (writes durable rows) |
| `file-io` | filesystem watches, vault subscribers, file operations | routed file/folder requests from UI | per-window `FileIoEvent`; vault fan-out events |
| search work | none (stateless helper calls) | query text + rope snapshots | match vectors to UI callers |

## Layer graph

Strict bottom-up. No cross-layer `pub use`.

```
text · win                                       # leaves, no deps
buffer ← text                                    # Buffer aggregate
engine ← buffer · text                           # synchronous state/edit/undo; no I/O
host ← engine · buffer · text                    # normalized intents/operations/events
persist ← buffer                                 # SQLite, edits, snapshots, backup
decorate ← buffer                                # tree-sitter, markdown spans
search ← buffer                                  # literal/regex find + fuzzy scoring
display_map ← buffer · decorate                  # source↔display projection
wasm ← host · engine · decorate · display_map · buffer # thin raw binding; no I/O
web component ← wasm                             # semantic browser input + DOM projection
core ← host · engine · buffer · persist · text   # native actor + durability adapter
command ← host · core · text · buffer            # registry + typed editor operations
keymap ← command · input                         # TOML chord lookup
theme · config                                   # TOML loaders + watcher
layout ← win                                     # DirectWrite layout cache
render ← layout · win · display_map              # D3D11 + DXGI + D2D + DWrite
ui ← render · command · keymap · core · display_map · …
app ← ui · core · persist · command · keymap     # only fn main; only `anyhow`
test_fixtures                                    # dependency-free semantic corpus
test_support ← core · ui · persist · …           # native fixtures/harnesses
xtask                                            # workspace tasks
```

Owner reminders:
- each `Engine` has one caller-selected mutable owner; Windows uses `core`.
- each Web Component and its engine are owned by one JavaScript agent; the
  component creates no worker or persistence service.
- `ui` is the only owner of HWNDs.
- each `EditorSurface::render` is UI-thread-owned and contains the native
  renderer, DirectWrite format/factory, layout cache, and projection-walker
  cache handles; HWND client pixels and DPI/resize events remain host-adapter
  state on `Window` or `EditorControl`.
- `EditorSurface::focus` owns editor-control keyboard focus and nested overlay
  input focus; application foreground activation remains host state on
  `Window` because a child editor can change focus without changing its host's
  activation.
- `EditorSurface::selection` owns UI-only motion intent and last-edit
  navigation memory; `Engine` remains selection truth.
  `EditorSurface::selection_dispatch` coordinates pre/post surface-local edit
  effects while one native selection adapter applies desktop autosave,
  projection, edit-pulse, and persistence-status effects.
  `EditorSurface::clipboard` owns ephemeral paste history and canonical text
  normalization. Plain text crosses the surface as `HostRequest`; the native
  clipboard adapter resolves it with `continuity_win`, which exclusively owns
  Windows clipboard handles and format I/O. HTML, DIB, and dropped-file formats
  remain native extensions.
- `EditorSurface::accessibility` is published by the UI thread and read through
  short immutable snapshots by UI Automation. Provider-triggered selection
  changes are sent back to the owning UI thread before the engine mutates.
- `app` is the only crate with `fn main`.

## Hot paths

### Keystroke → durable
1. UI thread `on_char` / `on_keydown` → keymap lookup → `dispatch_command`.
2. Command handler builds a `SelectionEdit` and calls `Context::apply_selection_edit`.
3. `Window::dispatch_selection_edit` sends
   `EditorMessage::ApplySelectionEdit { operation: EditorOperation }` over the
   crossbeam channel.
4. Core applies the typed operation through `host::apply_editor_operation`;
   the engine plans,
   mutates, updates undo/deltas, and returns `ChangeBatch`.
5. Core's `PersistenceBridge` assigns edit-log sequence numbers, encodes the
   batch, enqueues rows for persist, then emits `EditEvent::EditApplied`.
6. Persist thread batches every 250 ms or 64 KiB; commits one transaction.

Budget: keystroke → pixel ≤ 8 ms p99; edit → durable ≤ 400 ms p99.

### Edit → paint
1. Core emits `EditEvent::EditApplied { id, revision }`.
2. UI invalidates affected `EditorSurface::render` layout-cache lines + posts
   `WM_PAINT`.
3. UI submits a decoration request `(BufferId, Revision)` to the worker pool.
4. On `WM_PAINT`: build `FrameDisplay` projection from latest snapshot + decoration cache.
5. Render frame; cached `IDWriteTextLayout`s reused when revision matches.

Stale decoration results that arrive with `revision < buffer.revision` are discarded — no callbacks, no locks.

For `EditorControl`, normalized input synchronously returns a `HostEventBatch`
from its caller-thread-owned runtime, delivers the batch after the engine
borrow ends, invalidates the child, and paints through the same
`EditorSurface::render`/`FrameDisplay` path. Persistence acknowledgement is
entirely host-owned.

### File save
1. UI dispatches `file.save` → `Window::file_save_impl`.
2. If `editor.trim_trailing_whitespace_on_save` on, fire `SelectionEdit::TrimTrailingWhitespaceAll` (one undo group).
3. Snapshot the rope, hand the bytes + path to the file-I/O thread.
4. File-I/O writes atomically (temp file + rename), then `FileIoEvent::Saved` to UI.
5. UI updates the file association mtime/hash and shows a banner.

Vault continuous export uses the same path and conflict guard, but scheduling is per-window UI state and successful automatic saves are silent. The file-I/O thread additionally owns nearest-marker discovery, marker watches, shallow config-aware listings, and contained create/move/Recycle-Bin operations. `config` owns only the parsed `VaultConfig`; `app` owns Preview/NewTab/NewWindow routing. See `features/vaults.md`.

## Invariants

- Every cross-thread payload is an `Arc<…>` clone tagged with `Revision`. No `&'a` lifetimes crossing channels.
- `Mutex` is allowed only where a doc comment names the contention region (theme cache, font cache). Hot paths are lock-free.
- Channels are bounded outside startup; hot-path sends are `try_send` with explicit overflow policies (e.g. coalesce-on-full in persist).
- A panic in any worker is caught at the crate boundary, logged, and the worker restarts.
- **UI thread panic quarantine (implemented).** Win32 calls `crates/ui/src/window_dispatch.rs::wndproc` across an `extern "system"` FFI boundary; an unwind crossing it is UB that aborts the process, and the shipped `release-small` profile compiles `panic = "unwind"`. The routing body runs inside `std::panic::catch_unwind`; a caught panic is logged and converted to a safe `LRESULT` by `crates/ui/src/window_dispatch/panic_barrier.rs::recover_from_wndproc_panic` (`LRESULT(0)` for messages the dispatch treats as handled, else `DefWindowProcW`), so the window survives the faulting message instead of aborting. Best-effort: it preserves process survival over a single message, not transactional rollback — `Window` state may be left mid-mutation, so the conservative `is_handled_message` set falls unknown messages through to the OS default.

## Constraints + trade-offs
- **Win32 raw, not winit** ⇒ full control over IME, VD, DPI, swap-chain present ⇒ Windows-only.
- **DirectWrite/Direct2D** ⇒ best on-Windows text quality ⇒ no `wgpu` cross-platform layer.
- **Sync threads, no async** ⇒ deterministic latency, no executor overhead ⇒ no `tokio` ecosystem reuse.
- **Synchronous storage-neutral engine** ⇒ embedded callers can use ephemeral
  state directly; desktop SQLite durability stays a native host adapter.
- **Single-binary, no plugin runtime** ⇒ native Windows executable ≤9 MiB stripped, no sandbox ⇒ extension model is fork + recompile.

## Failure modes
- **Decoration revision mismatch** ⇒ result discarded silently, next paint uses cached `Decorations` ⇒ next worker pass picks up.
- **Persist queue > 8 MB unflushed** ⇒ core coalesces adjacent inserts/deletes per buffer + undo group before forwarding ⇒ UI thread never blocks on disk.
- **Snapshot checksum corrupt** ⇒ fall back to previous snapshot; if needed again, halt replay at first bad row and present a recovery banner ⇒ never silently lose edits.
- **Decoration worker panic** ⇒ caught at pool boundary, worker re-spawned, line keeps the last-known good decoration ⇒ editor stays usable.
- **Virtual desktop GUID gone** ⇒ window restores onto the active desktop ⇒ no auto-switch, no focus theft.

## References
- `.docs/development/archive/spec.md` §§1–4 (historical stack, threading,
  buffer, and persistence rationale).
- `.docs/development/code_organization.md` (full layer graph + abstraction rules).
- `.docs/design/concurrency.md` (channel topology details).
