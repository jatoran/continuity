# Architecture

## Scope
- In: runtime topology, thread roles, layer graph, ownership rules.
- Out: per-feature internals (see `features/*`), Win32 message dispatch details (`technical/paint-flow.md`).

## Vocabulary
- **Core thread**: the singleton owner of every `Buffer`'s mutable state.
- **UI thread**: per-window Win32 message pump + render submission.
- **Decoration worker**: pool member that turns `(RopeSnapshot, Revision)` into `Decorations`.
- **Persistence thread**: SQLite-connection owner, drains the bounded edit queue.
- **File-I/O thread**: reads/decodes imported files, writes exports.
- **Snapshot**: an `Arc<Rope>` (cheap clone) plus its `Revision` stamp. Only thing that crosses thread boundaries.

## Process model

Single process, multiple top-level Win32 windows, one shared editor core. Per window: own message pump, own swap chain, own pane tree, own scalar `view_options`. Cross-window shared: `EditorHandle`, persistence client, decoration pool, theme set, command registry, settings watcher.

**Single instance per data dir.** The process holds a named mutex keyed by the database path (`win::single_instance::SingleInstanceMutex`). A second launch is *not* a second process running the full session — it forwards its command-line file/folder paths to the running instance over a message-only `WM_COPYDATA` hub (`win::single_instance::InstanceHub`, spawned only by the mutex-holding primary) and exits; a bare relaunch just activates the running instance's top-most window. Only when no live instance is reachable does the launcher run standalone. `--new-instance` (and the `CONTINUITY_E2E_INSERT` test hook) bypass the handoff. This is what keeps a double-click / shortcut launch from replaying the persisted window set and duplicating every open window. Claim/forward logic: `app::single_instance::claim_or_forward`; on hub receive, forwarded **files** route through `RegistryEvent::OpenFileBuffer` (same path as in-process opens — dedup, reveal-existing-tab-or-spawn, and reconcile against current disk bytes; see [file-io](features/file-io.md) §Reconciliation), forwarded **folders** through `RegistryEvent::Spawn`, and a bare relaunch through window activation — all on the hub's pump thread.

## Thread map

| Thread | Owns | Reads | Sends |
|---|---|---|---|
| `core` | `EditorState` (`BufferId → Buffer`), undo trees | nothing on hot path | `EditEvent` broadcast, `EditRecord` to persist |
| `ui-window-N` | `HWND`, swap chain, `Window` struct, pane tree | `Arc<RopeSnapshot>`, `Arc<Decorations>`, `Arc<DisplayMap>` | `EditorMessage` to core, paint to D2D |
| `decorate-worker-K` (pool) | nothing | `(RopeSnapshot, Revision)` request | `DecorateResult` back to UI |
| `persist` | SQLite conn, edit queue | `EditRecord` from core | nothing (writes durable rows) |
| `file-io` | open file handles | file paths from UI | `FileIoEvent` to UI |
| search work | none (stateless helper calls) | query text + rope snapshots | match vectors to UI callers |

## Layer graph

Strict bottom-up. No cross-layer `pub use`.

```
text · win                                       # leaves, no deps
buffer ← text                                    # Buffer aggregate
persist ← buffer                                 # SQLite, edits, snapshots, backup
decorate ← buffer                                # tree-sitter, markdown spans
search ← buffer                                  # literal/regex find + fuzzy scoring
display_map ← buffer · decorate                  # source↔display projection
core ← buffer · persist · text                   # SOLE writer of buffer state
command ← core · text · buffer                   # registry + Context + predicates
keymap ← command · input                         # TOML chord lookup
theme · config                                   # TOML loaders + watcher
layout ← win                                     # DirectWrite layout cache
render ← layout · win · display_map              # D3D11 + DXGI + D2D + DWrite
ui ← render · command · keymap · core · display_map · …
app ← ui · core · persist · command · keymap     # only fn main; only `anyhow`
test_support ← buffer · text · persist           # fixtures, FakeClock, gens
xtask                                            # workspace tasks
```

Owner reminders:
- `core` is the only writer of buffer state.
- `ui` is the only owner of HWNDs.
- `app` is the only crate with `fn main`.

## Hot paths

### Keystroke → durable
1. UI thread `on_char` / `on_keydown` → keymap lookup → `dispatch_command`.
2. Command handler builds a `SelectionEdit` and calls `Context::apply_selection_edit`.
3. `Window::dispatch_selection_edit` sends `EditorMessage::ApplySelectionEdit` over crossbeam channel.
4. Core thread plans (`crate::selection_edit::plan`) → applies ops → bumps revision → emits `EditEvent::EditApplied`.
5. Core enqueues `EditRecord` for persist (bounded, byte-accounted).
6. Persist thread batches every 250 ms or 64 KiB; commits one transaction.

Budget: keystroke → pixel ≤ 8 ms p99; edit → durable ≤ 400 ms p99.

### Edit → paint
1. Core emits `EditEvent::EditApplied { id, revision }`.
2. UI invalidates affected layout-cache lines + posts `WM_PAINT`.
3. UI submits a decoration request `(BufferId, Revision)` to the worker pool.
4. On `WM_PAINT`: build `FrameDisplay` projection from latest snapshot + decoration cache.
5. Render frame; cached `IDWriteTextLayout`s reused when revision matches.

Stale decoration results that arrive with `revision < buffer.revision` are discarded — no callbacks, no locks.

### File save
1. UI dispatches `file.save` → `Window::file_save_impl`.
2. If `editor.trim_trailing_whitespace_on_save` on, fire `SelectionEdit::TrimTrailingWhitespaceAll` (one undo group).
3. Snapshot the rope, hand the bytes + path to the file-I/O thread.
4. File-I/O writes atomically (temp file + rename), then `FileIoEvent::Saved` to UI.
5. UI updates the file association mtime/hash and shows a banner.

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
- **Single-binary, no plugin runtime** ⇒ ≤8 MB stripped, no sandbox ⇒ extension model is fork + recompile.

## Failure modes
- **Decoration revision mismatch** ⇒ result discarded silently, next paint uses cached `Decorations` ⇒ next worker pass picks up.
- **Persist queue > 8 MB unflushed** ⇒ core coalesces adjacent inserts/deletes per buffer + undo group before forwarding ⇒ UI thread never blocks on disk.
- **Snapshot checksum corrupt** ⇒ fall back to previous snapshot; if needed again, halt replay at first bad row and present a recovery banner ⇒ never silently lose edits.
- **Decoration worker panic** ⇒ caught at pool boundary, worker re-spawned, line keeps the last-known good decoration ⇒ editor stays usable.
- **Virtual desktop GUID gone** ⇒ window restores onto the active desktop ⇒ no auto-switch, no focus theft.

## References
- `.docs/development/spec.md` §§1–4 (stack, threading, buffer, persistence).
- `.docs/development/code_organization.md` (full layer graph + abstraction rules).
- `.docs/design/concurrency.md` (channel topology details).
