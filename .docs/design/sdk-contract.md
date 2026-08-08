# SDK product and compatibility contract

This document freezes the public product coordinates and compatibility policy
for the embeddable Continuity family. It names intended SDK surfaces; it does
not claim that an SDK package ships before its roadmap milestone is complete.
`EMBEDDING.md` is the public human/agent integration router; changes to this
contract must keep that guide current in the same diff.

## Product coordinates

| Surface | Public coordinate |
|---|---|
| Rust engine package | `continuity-engine` |
| Rust crate/import | `continuity_engine` |
| npm package | `@continuity-editor/editor` |
| Web Component | `<continuity-editor>` |
| Native library | `continuity_engine` |
| C ABI symbol prefix | `continuity_engine_` |
| Python distribution | `continuity-editor` |
| Python import | `continuity_editor` |

The npm package contains the stable TypeScript facade, Web Component, and WASM
artifact as one installable unit. Raw generated WASM bindings are internal.
The C ABI and Python binding are gated Windows x86-64 preview artifacts. The
selected Python host story is headless automation, language tooling, and
applications that own storage and presentation. The wheel is not advertised
as a Qt/Tk/GTK widget; a Python GUI that wants Continuity's visual surface
hosts the separately distributed Web Component in a named web-view toolkit.

The Rust-native Windows visual surface now exists as
`continuity_ui::EditorControl`. It is source-available workspace API rather
than a published SDK artifact. Milestone 9 packages the storage-neutral
`continuity-text` → `continuity-buffer` → `continuity-engine` closure; it does
not claim the Win32 control as a registry-distributed visual SDK.

Registry names and organization scopes must be controlled by the Continuity
release owners, never by an individual maintainer account as the sole owner.
Registry publication is authorized only through the protected SDK release
workflow after the activation checklist is complete. Before the first preview
publication, release owners must verify registry ownership and availability.
If the npm organization scope or an unscoped registry name
cannot be secured, changing the coordinate requires one reviewed update to
this contract and every package fixture; silent aliases are not allowed.

## License and source ownership

SDK code and published packages use the workspace MIT license. The private
monorepo remains the source of truth until public staging is extended for SDK
artifacts. Generated bindings, declarations, headers, and wrapper versions are
derived from that source; a registry archive is never an independent source
repository.

## Engine and host boundary

The engine is synchronous and storage-neutral. Engine-owned buffers,
selections, revisions, and undo state mutate on the thread selected by the
host. The engine does not create a database, settings directory, worker,
window, message loop, or filesystem state. It never invokes a host callback
while mutable engine state is borrowed; revisioned events are delivered after
the mutation returns.

Hosts own persistence, windows, rendering, text measurement, clipboard, IME,
accessibility, and scheduling. Ephemeral use requires no persistence adapter.
The native Windows application remains a durable host: its core actor is the
sole buffer writer, its persistence thread retains SQLite/WAL/recovery/backups,
and its UI thread retains HWND and DirectWrite/Direct2D ownership. Extracting
the engine does not remove desktop persistence.

The synchronous Rust implementation now exists in `crates/engine`. Its public
facade returns `ChangeBatch` after each mutation and drains lightweight events
explicitly. The publishable minimum closure is `continuity-text` →
`continuity-buffer` → `continuity-engine`; extracted `.crate` archives build a
clean consumer. Registry publication remains an owner-approved, protected
environment step; local xtask commands never upload.

`crates/c_api` builds `continuity_engine.dll` with ABI major 1. Handles are
confined to their construction thread and destroyed exactly once there. Rust
owns returned strings and arrays until their matching free call. UTF-8 source
positions are byte-based; UTF-16 inputs reject unpaired surrogates. Callbacks
receive only a revision after mutation completes, and same-handle calls during
a callback are rejected. `release-sdk` retains unwinding so exported functions
contain panics before they reach C.

`bindings/python` builds the `continuity-editor` CPython 3.10+ `abi3` wheel.
Each `Editor` is construction-thread confined, storage-neutral, explicitly
closeable, and callback-capable. The extension creates no filesystem,
database, worker, or widget state.

The synchronous WASM implementation now exists in `crates/wasm` and the stable
facade in `packages/editor`. `cargo xtask wasm-check` installs the produced npm
tarball into a clean consumer and runs the shared parity corpus and budgets.
The artifact is staged by the unified SDK release lane; it is not publicly
available until registry activation succeeds. The Web Component is supported
first on the Chromium/Electron lane;
Firefox and WebKit remain unsupported until they have equivalent evidence.

The same npm artifact exports `@continuity-editor/editor/react` as an optional
React 18/19 adapter. React is an optional peer so framework-neutral and headless
consumers do not install it. The adapter's `value`/`revision` pair is a complete
Continuity snapshot; host persistence revisions remain a separate contract.

The component uses a semantic textarea plus DOM display projection. It emits
versioned, component-local sequenced change batches only after synchronous
engine mutation returns. A restoring host supplies both canonical text and its
last accepted revision before readiness; revisions do not reset across a
durable host restart. Hosts own ordered persistence and durability. Link,
context-menu, and dropped-file work is host-mediated. The Electron example
keeps Electron/Node APIs in a context-isolated preload and main process; none
enter the editor package.

Portable command identifiers resolve in the storage-neutral `host` layer and
execute synchronously through the same typed operations as native Windows.
The Web Component defaults to a `browser-safe` shortcut policy, offers an
`editor-first` policy for controlled shells, and lets a host replace or unbind
individual chords. A regular browser remains free to withhold browser-UI
accelerators before page dispatch; direct `executeCommand` calls are the
deterministic integration path when a chord is unavailable.

`crates/host` defines the internal binding contract and the context-free
command-name resolver. `EditorOperation` contains
only editor-owned mutations; desktop files, panes, tabs, windows, settings, and
application lifecycle are not representable there. `EditorIntent` normalizes
logical navigation, selection, viewport, scroll, focus, pointer, composition,
command ownership, and host mediation independently of Win32 messages.
Pointer intents use surface-DIP coordinates and carry lifecycle phase, active
and held buttons, click count, and normalized modifiers. Plain-text clipboard
reads and writes are host requests; native rich clipboard formats are
platform-specific extensions rather than part of the portable contract.

Each `HostRuntime::dispatch` returns exactly one monotonically sequenced
`HostEventBatch`. Events are causally ordered within the batch and may include
the full `ChangeBatch`, normalized selections, focus/viewport changes,
invalidation, clipboard/context-menu/link requests, or recoverable banner
data. A binding delivers that batch only after `dispatch` returns. It may then
synchronously dispatch a new intent; there is no callback entry point capable
of reentering a live mutable engine borrow.

Runtime calls are confined to the construction thread. Dispatch after teardown
is rejected. Revision-guarded operations fail before mutation when the live
revision differs. UTF-8 byte and UTF-16 code-unit conversion rejects offsets
inside a scalar value or surrogate pair rather than rounding them.

The Windows child control delivers each returned batch through a bounded
`ControlEventSink`. Its host owns the receiver and persistence acknowledgement;
the control applies lossless channel backpressure and reports disconnection.
See `.docs/design/features/embeddable-windows-control.md` for HWND, focus,
resize/DPI, clipboard, IME, accessibility, and teardown ownership.

## Host durability and backpressure

Returning from an engine mutation acknowledges only an accepted in-memory
change. It does not mean a host has saved, flushed, or durably committed the
batch. A host may report crash durability through revision `R` only after its
storage adapter has acknowledged that every ordered batch through `R` is on
durable media according to that adapter's documented guarantee.

Hosts may debounce full snapshots or ordinary document exports, but they must
not reorder or silently drop accepted operation batches. When a host queue is
full it must apply explicit backpressure, spill to a durable queue, or surface
a recoverable durability error; it may not continue claiming durability for
unacknowledged revisions. Host code processes returned batches and drained
events only after the mutable engine call has ended and must not re-enter the
same engine from an event handler.

The Windows adapter retains its existing contract: `ChangeBatch` rows enter
the bounded persist queue in order, SQLite owns the commit, write failures and
thread loss become sticky banners, and measured edit-to-durable latency must
remain at or below 400 ms p99. Queue acceptance is not itself a durable-media
acknowledgement.

## Supported-target policy

The native Windows application is the only shipped end-user product. The
headless WASM engine is also a gated build artifact, but is not registry
published and does not imply browser presentation support.

| Tier | Target | Current claim / required evidence |
|---|---|---|
| 1 | `x86_64-pc-windows-msvc` native engine/control | full Windows CI, parity corpus, native control gates |
| 1 | `wasm32-unknown-unknown` engine | compile/size/parity/packed-consumer gates are active; no registry claim |
| 1 | Web Component on Chromium/Electron | packed browser contract, accessibility tree, IME/input, rendering budgets, and Electron IPC E2E are active; manual Windows screen-reader acceptance is required before release |
| 1 | Windows x86-64 C ABI major 1 | checked header, unwind-enabled DLL, MSVC external consumer, parity/callback/teardown gates |
| 1 | Windows x86-64 CPython 3.10+ | `abi3` wheel, clean-venv install, parity/callback/snapshot/teardown gates |
| 2 | `aarch64-apple-darwin`, `x86_64-apple-darwin` engine | native engine CI and packed consumer |
| 2 | `x86_64-unknown-linux-gnu` engine | native engine CI and packed consumer |
| Provisional | Windows ARM64, Linux ARM64, other browsers | explicit CI, artifact, and host evidence |

The eventual web-shell desktop application has its own OS support matrix. It
does not broaden the native DirectWrite child control beyond Windows.

## Toolchain and stability

- Rust MSRV is the workspace `rust-version`, currently 1.89. Raising it is a
  documented SDK compatibility change with an MSRV CI update.
- Production development follows stable Rust. Nightly-only public API or build
  requirements are forbidden.
- SDK releases begin at `0.1.0` as preview software. Before `1.0`, documented
  public APIs may change in a minor release; patch releases are compatible bug
  fixes. After `1.0`, SemVer applies normally.
- The Rust facade, TypeScript facade/Web Component, C ABI, and Python facade are
  public contracts. Internal crate paths, raw WASM exports, SQLite schema, and
  Win32 window internals are not SDK APIs.
- C ABI major 1 is additive within the major: new functions, status values,
  struct tail fields, and capability bits require a minor bump. Removing or
  changing an existing symbol, field, allocator rule, or status meaning
  requires ABI major 2. Callers negotiate the major and inspect `struct_size`
  plus capability flags.
- A deprecated Rust, C, or Python API remains available for at least one SDK
  minor release and names its replacement in the package changelog. Removal
  before 1.0 occurs only in the next minor; after 1.0 it requires a SemVer
  major release. Compatibility fixtures compile retained surfaces.

## Compatibility gates

Every implementation consumes the checked-in parity corpus. A release is
blocked by drift in text, selections, revisions, multi-cursor behavior, no-op
edits, typing coalescence, undo branches, rope deltas, Markdown decoration, or
source/display mappings unless the contract and all consumers change together.

Serialized host events and saved host-owned operation batches carry an
explicit protocol version. Unknown major versions are rejected; unknown
optional fields within a supported major are ignored. C callers negotiate an
ABI version and capability set, and no Rust panic may cross the ABI boundary.

## Version trains

Desktop and SDK versions are deliberately independent. The native desktop is
currently `0.4.2`; the first consumable SDK preview starts at `0.1.0`. A
desktop release records the minimum compatible SDK engine version it embeds,
but a desktop feature release does not force a registry release.

All artifacts in one SDK release share `sdk/release.toml` as the canonical SDK
version. Cargo, npm, C header/library metadata, and any Python wheel must match
it mechanically through `cargo xtask sdk-release-check`.
Bindings may be omitted from a release, but a binding published for that
release may not carry an unrelated version.

## Change control

Breaking the engine/host ownership model, public coordinates, license, target
tiers, MSRV, stability rules, protocol policy, or version-train relationship
requires a design-doc change before implementation. Package availability alone
does not justify forking editor logic or exposing an internal crate directly.
