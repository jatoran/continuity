# Concurrency

## Scope
- In: thread roles, channel topology, snapshot discipline, locks-of-last-resort.
- Out: per-feature mutex rationale (see each feature doc), buffer revisions internals (see `features/buffer.md`).

## Vocabulary
- **Single-writer**: exactly one thread holds the `&mut` to a domain's mutable state.
- **Snapshot**: an `Arc<T>` clone tagged with `Revision`, safe to share across threads.
- **Revision**: monotonically increasing `u64` per buffer. Workers stamp their results; consumers drop results whose revision is stale.

## Ownership map

| Domain | Owner thread | Cross-thread view |
|---|---|---|
| Buffer text + selections + undo | caller-selected engine owner (Windows: `core`) | `Arc<RopeSnapshot>` (immutable, revision-stamped) |
| SQLite connection | `persist` | none — talk through `PersistClient` (clonable `Sender`) |
| Desktop message pump + HWND + swap chain | `ui-window-N`; `DesktopShell` owns the pump, `Window` composes HWND/render state | none — UI delivers paint via the wndproc |
| Embedded parent pump + child HWND/runtime/swap chain | host-selected UI thread; host owns pump, `EditorControl` owns child surface and `HostRuntime` | bounded ordered `HostEventBatch` receiver owned by host |
| WASM editor state + synchronous projection | constructing JavaScript agent; npm `Editor` owns one WASM `Engine` | returned JSON-compatible reports only; no internal worker or callback |
| Editor input + pointer/selection interaction + focused viewport + caret presentation + projection caches | `ui-window-N`; `EditorSurface` is the sole writer | immutable rope/decorations snapshots enter the projection worker |
| Decoration cache | `ui-window-N` (writer); workers produce | `Arc<Decorations>` |
| Display map | `ui-window-N` builder (synchronous in paint) | `Arc<DisplayMap>` consumed by render |
| Layout cache | `ui-window-N` (LRU keyed by `(buffer, line, rev, font, wrap)`) | not shared |
| Projection worker | `projection-worker-N` (one per `EditorSurface`, joined on surface drop) | `ProjectionRequest` in via crossbeam-channel; latest-result cell out. Paint polls once and never waits; worker publication only upgrades a paint when the result is already ready. |
| Theme | shared `Arc<Theme>` switched atomically on hot reload | reads are lock-free clones |
| Settings | shared `Arc<Settings>` updated by config-poll on UI threads | reads via `apply_settings` snapshots |

## Channel topology

All channels are `crossbeam-channel`. Bounded unless explicitly justified.

```
UI(N)  ──EditorMessage─────►  core
                              │
core   ──EditEvent────────►   UI(1..N)         (broadcast subscribers)
core   ──encoded ChangeBatch rows► persist      (bounded, byte-accounted, 8 MB cap)
UI(N)  ──DecorateRequest──►   pool
pool   ──DecorateResult───►   UI(N)
UI(N)  ──FileIoRequest────►   file-io
file-io──FileIoEvent──────►   UI(N)
UI(N)  ──ProjectionRequest─►  projection-worker-N      (latest-wins; bound 64; coalesced on drain)
projection-worker-N ──ProjectionResult──► UI(N)        (Mutex<Option<…>> + Condvar; new result overwrites prior, condvar notifies waiting paint)
persist──ack/none           (writes durable rows; no upstream replies on hot path)
EditorControl ──HostEventBatch──► embedding host (bounded, lossless backpressure)
```

`ProjectionResult` carries `seq: u64`, `stamp: ProjectionStamp`, `frame_display: FrameDisplay`, plus latency fields `build_dur_us: u64` (wall time inside `build_for_request`) and `coalesced_dropped: u32` (queued requests this build replaced). UI surfaces both on the worker_hit trace so a slow worker vs. a fast-producer UI thread are distinguishable. Stale-stamp rejections also carry the stale stamp + latency so the trace can attribute the freshness gap (`event:projection_worker_stale_result`).

Production paint does not do a paint-time worker wait. The worker remains the sole writer of the latest-result cell, and the UI thread remains the sole reader via `take_latest_result`, but `Window::resolve_paint_frame_display` does one non-blocking poll per paint and immediately uses the cache/inline/partial fallback on miss. See `.docs/technical/paint-flow.md` § "No Paint-Time Worker Wait" for the current latency contract.

`EditorMessage` carries a reply `Sender` for synchronous results (e.g. `Snapshot`, `ApplySelectionEdit { reply: Sender<Result<Option<Revision>, Error>> }`).

Hot-path producers use `try_send` with explicit overflow policies:
- Persist queue full → core coalesces adjacent inserts/deletes per `(buffer_id, undo_group)` before re-trying.
- Decoration request flood → only the most recent request per buffer is retained.

## Revision discipline

Every `EditEvent::EditApplied` carries the new `Revision`. Every async worker output carries the revision it was computed against. The UI keeps the latest accepted revision per buffer; results stamped older are dropped.

```rs
// pseudo
if result.revision == window.last_known_revision(buffer_id) {
    window.decoration_cache.insert(buffer_id, result.decorations);
}
// else: silently drop
```

This is the entirety of the "should I accept this result?" logic. No locks, no callbacks.

### Rope-delta history is revision-truthful

Every revision-advancing engine path records its per-revision rope deltas so
`EditorHandle::rope_deltas_since` and direct `Engine::deltas_since` return a
gap-free chain. Forward edits, undo, redo, and alternate redo all capture
point-augmented deltas against the pre-apply rope in engine application order.

## Locks of last resort

Allowed only with a doc comment naming the region. Current allowlist:
- `theme::ActiveTheme` inner: cached resolved palette behind `Mutex`. Updated by `theme.reload`; readers clone the `Arc<Theme>` snapshot, so the lock is short.
- DirectWrite font collection cache: `Mutex` to amortize collection enumeration cost across windows.
- (no others on the hot path)

Forbidden:
- `Mutex` on `Buffer` state, `Decorations`, `DisplayMap`, layout cache, undo tree.

## Buffer single-writer

One caller-selected thread owns each `Engine`. In the Windows application that
owner is the core actor and every UI mutator flows in via `EditorMessage`.
Embedded hosts may instead keep `Engine` directly on their chosen thread.
`HostRuntime` records its construction thread and rejects wrong-thread or
post-teardown calls. It exposes no callback-taking mutation method: one
dispatch returns one complete event batch after all engine borrows end.
`EditorControl` carries an `Rc` affinity marker and therefore cannot cross
threads. Its event channel can cross threads, but the HWND/runtime/surface
remain on the host pump thread. Child destruction never terminates that pump.

The WASM adapter applies the same rule without channels: its `Editor` remains
on the constructing JavaScript agent, every call completes synchronously, and
the caller receives the report only after the mutable engine borrow ends. Web
Worker transfer and scheduling are intentionally absent in Milestone 7.

Each native `EditorSurface::render` is owned and mutated by its UI thread.
DirectWrite formats/factories, the D2D/DXGI renderer, and the layout cache do
not cross that boundary. The row-count run/wrap/segment caches use shared
backing stores only because the surface projection worker reads and populates
them from immutable projection requests; the owning handles and renderer
lifecycle remain surface state.

### Snapshot hand-off

```rs
let snap = self.editor.snapshot(buffer_id)?;
// snap.rope is Arc<Rope>; snap.selections is Vec<Selection>; snap.revision is the stamp.
// Use freely on any thread until next snapshot.
```

The UI re-snapshots on every paint frame. Cost is `O(Arc clone)` — `ropey::Rope`'s `Arc` interior makes this nearly free.

## Edit pipeline

```
UI dispatch
   └─► SelectionEdit (typed payload)
         └─► EditorOperation::ApplySelectionEdit(edit)
                └─► EditorMessage::ApplySelectionEdit { operation, reply }
                └─► [core] host::apply_editor_operation(operation, timestamp)
                       └─► engine::selection_edit::plan → SelectionEditPlan
                       └─► engine undo manager
                              ├─► for each op: buf.apply(op)  (auto-transform of existing selections)
                              ├─► undo_tree.append_record(group_id, …)
                              └─► return ordered ChangeBatch
                       └─► buf.set_selections(plan.selections_after)
                └─► core PersistenceBridge assigns seq + enqueues SQLite rows
                └─► reply.send(Ok(Some(new_revision)))
   ◄── (UI invalidates, schedules WM_PAINT, queues decoration request)
```

`SelectionEdit` is the named-edit entry point; raw/grouped engine edits are the
other explicit mutation contracts.

## Backpressure

| Producer | Consumer | Capacity | On full |
|---|---|---|---|
| UI → core | core inbox | 256 messages | block UI briefly (≤1 frame); never drop |
| core → persist | persist queue | 8 MB byte budget | coalesce adjacent ops in-place; never drop |
| core → event subscribers | unbounded | n/a | subscriber must drain; slow subscribers are dropped at the broadcast layer |
| UI → decorate pool | one slot per buffer | 1 | replace pending request (newest wins) |
| UI → file-io | 16 messages | 16 | block UI; this is acceptable for file operations |

## Failure modes
- **Channel closed** ⇒ `Err(SendError)` bubbles as `Error::CoreUnavailable`; UI shows a banner, refuses further input until restart.
- **Reply channel never fires** ⇒ blocking call returns `Error::ReplyDropped` after watchdog timeout (10 s). Never indefinite.
- **Snapshot taken mid-edit** ⇒ impossible — the rope mutates only while its engine owner holds `&mut Engine`.

## Constraints + trade-offs
- **No async runtime** ⇒ deterministic latency, no executor scheduler ⇒ no `tokio::fs`, no `reqwest`, no `serde_yaml` (yaml's parser is async).
- **Bounded channels everywhere** ⇒ predictable memory, observable backpressure ⇒ producers must implement overflow policy explicitly.
- **No `Mutex<Buffer>`** ⇒ the compiler enforces single-writer; native calls
  round-trip through core, direct hosts call their owned engine.

## References
- `.docs/development/archive/spec.md` §2 (threading) + §3 (historical buffer
  model rationale).
- `.docs/design/features/buffer.md` for `Revision` invariants.
- `crates/core/src/handle.rs` for the message dispatch.
