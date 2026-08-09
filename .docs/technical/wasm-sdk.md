# WASM engine and npm package

## Scope

Milestones 7 and 8 supply a synchronous WebAssembly build of Continuity's
storage-neutral engine plus a framework-neutral `<continuity-editor>` browser
surface. Both ship in the packed `@continuity-editor/editor` npm artifact,
which is validated locally and in CI and included in the unified SDK release
bundle. Public npm availability remains gated on registry activation. The first
supported presentation lane is Chromium/Electron; Firefox and WebKit are not
claimed.

## Ownership and dependency closure

One JavaScript agent owns each `Editor` and its underlying `Engine`. Every
mutation, undo operation, Markdown decoration pass, and display-map build is
synchronous on that agent. The binding creates no worker, callback, database,
file, clock, window, event loop, or persistence queue. The host owns all of
those services and may discard returned changes for ephemeral use or persist
them after the mutation returns.

The portable closure is:

```text
text <- buffer <- engine <- host
          |
          +-> decorate -> display_map
wasm <- engine + host + display_map
```

Native crossbeam pools/watchdogs, SQLite, filesystem tracing, `PathBuf` host
metadata, Windows APIs, and desktop clocks are outside this closure. Synchronous
decoration and display projection remain shared Rust code rather than a second
JavaScript editor implementation.

`continuity-buffer` target-gates its desktop `FileAssociation` module, field,
constructors, and accessors on WASM; the portable `Buffer` therefore contains
no filesystem path metadata even before link-time dead-code elimination.

## Binding and package boundary

`crates/wasm` is a thin `wasm-bindgen` transport. Its generated `RawEditor` and
JSON methods are internal implementation details. `packages/editor/index.js`
and `index.d.ts` own the stable TypeScript facade:

- `initialize` loads the module once per JavaScript agent;
- `Editor` owns one ephemeral buffer;
- selections, insert, delete-backward, line indent/outdent, stable portable
  command execution, undo, redo, alternate redo, snapshot,
  projection, compact presentation, memory observation, and explicit teardown
  are synchronous;
- revisions and byte offsets are numbers; the full-width content checksum is a
  decimal string so JavaScript cannot lose `u64` precision;
- `projection` returns Markdown block/inline spans, display segments, and
  complete source/display mappings.
- `presentation` returns the same canonical projected text and segments without
  per-byte mapping arrays for DOM consumers. Change reports include ordered
  text splices so hosts can update mirrors without requesting a full snapshot.
- Each presentation line carries `wrapIndentByteEnd`, the shared display-space
  byte boundary after leading indentation and any list marker. The component
  measures that prefix with the active browser font for pixel-exact hanging
  indentation; the field is additive and does not change initialization or
  controlled-value contracts.
- `presentationRange(startLine, endLine)` materializes only a requested source
  window after the initial report; browser-owned no-wrap geometry does not
  require a whole-document display-row walk.
- `exportHistory({ maxGroups })` / `importHistory(history)` move the per-buffer
  undo tree across an unmount as plain JSON. Groups link by array index rather
  than by their internal UUIDs, so identity is re-minted on import and a
  truncated blob stays self-consistent. The blob carries the FNV-1a checksum of
  the rope it was taken from and an import into different content is refused:
  undo replays recorded inverse ops against the live rope, so a mismatch would
  rewrite the wrong bytes rather than fail. `maxGroups` keeps the newest groups
  and then widens the retained window until at most one retained group has a
  dropped parent — a redo from the pre-history head re-applies the most recent
  root branch, so two competing roots could re-apply ops against a rope that
  never held their pre-state.
- `ContinuityEditorElement` owns one `Editor`, exposes initial value, snapshot,
  initial-revision restoration, revision-checked replacement, read-only,
  focus, and teardown APIs, and emits
  versioned, sequenced changes and host requests only after mutation returns.
- `ContinuityEditor` from the `/react` subpath bridges React refs, attributes,
  custom events, and controlled `value`/`revision` reconciliation. It adds no
  editor state or persistence service.
- `attachContinuityEditor` from the `/controller` subpath provides the same
  initialization, configuration, event forwarding, snapshot de-duplication,
  revision-checked synchronization, and listener disposal without a framework
  dependency.
- `/svelte` and `/vue` provide controlled lifecycle adapters; `/lazy` is a
  dependency-free dynamic loader; `/conformance` exercises disposable host
  wiring; `/commit-queue` is an opt-in backend-neutral save scheduler.
- `<continuity-renderer>` provides static selectable Markdown/plain output,
  while `syntax="plain"` keeps the editable engine and disables projection.

The npm archive contains the MIT license, facade source/declarations, generated
internal loader/declarations, and optimized `.wasm`. Raw generated JavaScript
is deliberately not exported as a public API. The
`@continuity-editor/editor/wasm` package subpath exists so
Node and Electron hosts can read the packaged module bytes without depending
on package-manager installation paths.

## Web Component ownership and input

The component is a custom element with a shadow root. A transparent semantic
`textarea` owns browser editing semantics, native IME/composition, semantic
selection, keyboard navigation, clipboard integration, focus, and the
accessibility tree. On a coarse pointer the textarea stops receiving pointer
events: a transparent shield covers the editing area, takes the touch, and owns
scrolling, because platform touch selection hit-tests the textarea and that
layout cannot match the projection. Touch `pointerdown` records the projected
gesture without focusing, capture, or default prevention. Only a resolved tap
commits the projected caret and synchronously focuses the textarea in the
trusted click turn; pan, cancellation, and long-press ownership suppress that
focus path. This is component-owned default behavior and exposes no shadow-DOM
or host hit-test contract. Fine-pointer routing is unchanged. See
[Touch input](../design/features/touch-input.md). A DOM projection underneath it consumes Rust decoration
and display-map output for visible Markdown. An overlay
paints collapsed carets and non-collapsed ranges from the projection's measured
glyph rectangles; the raw-source textarea caret plus selected background and
foreground stay transparent so Chromium cannot paint a duplicate source layer.
IME composition is line-scoped: the projection stays visible, the composing
source line previews the live textarea text without any engine write, and the
overlay paints the live textarea caret and same-line selection against those
previewed glyphs. Mobile keyboards compose continuously (a composition per
word), so a frame-wide reveal would flash the whole document between raw source
and rendered Markdown on every word. Only a composed run that cannot be mapped
onto the projected line structure (for example one containing a newline) falls
back to a frame-wide native glyph, caret, and selection reveal.
This is the measured hybrid
presentation selected by the Milestone 8 spike; a canvas-only editor without a
semantic input bridge is forbidden.

`beforeinput` is normalized into synchronous engine insert/delete operations.
Physical Enter is claimed at `keydown` because Chromium reports ordinary Enter
and Shift+Enter with the same textarea line-break input type: Enter dispatches
the shared smart-newline planner and Shift+Enter inserts a raw newline.
Non-keyboard line-break input defaults to smart newline.
Composition is allowed to proceed in the textarea and reconciled once at
`compositionend` against the expected engine revision. Browser selection is
converted between UTF-16 code units and canonical UTF-8 byte positions.
Native textarea selection still owns keyboard movement and semantic state.
Arrow, Home, End, and Page navigation reconcile the semantic selection on the
next animation frame after each browser-default keydown, including repeats, so
the projected caret moves before any subsequent edit.
Simple clicks, Shift+click, and primary-button drags capture graphemes from the
actual rendered projection rectangles, map those display bytes through the
line's display-map segments, and commit the canonical source caret or range. This
avoids raw-source hit-test drift on wrapped WYSIWYG lines and remains exact for
host-selected proportional fonts, font sizes, zoom, and device scale.
Double-click and triple-click place that measured source caret before dispatching
`editor.select_word` or `editor.select_line` through the portable Rust host
operation boundary; JavaScript does not duplicate Unicode word or line-boundary
rules. Plain-text paste, cut, and drop mutate through the engine; links, context
menus, and dropped files become `continuity-request` events for the host.

A pointer tap taken while an IME composition is active obeys the
composition-to-pointer ownership rule (see
`.docs/design/features/web-component.md`): hit-testing maps against the live
textarea line for the source-visible composing line rather than the still
pre-composition `sourceLines` mirror, and the projected selection is retained and
replayed against the fresh snapshot only after `compositionend` reconciles the
textarea once. This is what keeps an Android predictive-keyboard tap on the
character under the finger instead of a stale-mapped byte; the fix carries no
keyboard, browser, or user-agent branch.

The same projection geometry positions the blinking primary caret, every
secondary caret, and visible selection rectangles after pointer or keyboard
movement, edits, scrolling, viewport restoration, and idle reconciliation.
Only the selection endpoint lines reveal source; intermediate lines keep their
Markdown projection, and range painting is bounded to the detailed viewport.
Host `revealRange` and `setSelections(..., { reveal: true })` requests are held until this render has landed.
The adapter first scrolls from the available projected geometry, realizes the new target viewport, then corrects from the detailed caret or range rectangles.
The correction is converted from projection-space offset to the current textarea or touch-shield scroll offset, including signed desktop scroll compensation when the projection is taller or shorter than the textarea.
Document edits reuse the projected caret point painted in that render and apply nearest alignment to the active scroll owner.
Large edit-driven jumps realize and correct the destination viewport, while local typing stays within the existing realized window.

Tab and Shift+Tab dispatch the shared Rust line indent/outdent planners using
the native default tab unit; they do not insert a character at the caret or
delegate indentation to the host. Escape arms one Tab/Shift+Tab traversal so
keyboard focus can leave, read-only mode releases Tab directly, and
`tab-behavior="focus"` is an explicit host opt-out. The semantic textarea's
accessible description advises the escape gesture.

Command shortcuts use three explicit policies:

- `browser-safe` is the default. It claims editor chords that do not overlap
  known Chromium UI and DevTools accelerators, and releases conflicts such as
  Ctrl/Cmd+E, K, R, Shift+R, J, Shift+J, U, and Shift+C. Ctrl/Cmd+Shift+S is
  also left free for a host-level Save As action.
- `editor-first` claims every delivered Continuity binding. A normal browser
  may still consume browser-UI shortcuts before the page receives `keydown`,
  so this is best suited to Electron or another controlled shell.
- `none` releases all default command chords for a host-owned keymap. The
  `shortcutBindings` property overlays any policy; a command string binds and
  `null` explicitly releases a chord. `executeCommand(command)` bypasses chord
  delivery while retaining the shared typed-operation path.

`listShortcutBindings()` exposes every default chord, command, and
`isBrowserSafe` classification. When policy releases a delivered built-in
binding, the element emits `continuity-shortcut-suppressed { version, chord,
command, policy }` without canceling the browser event. Explicit host overrides
are decisions rather than suppressions and therefore do not emit that event.

Bindings use logical `Mod` for Ctrl on Windows/Linux and Command on macOS;
`Ctrl`, `Meta`, `Alt`, and `Shift` are also accepted for exact overrides.
Browser-native selection, movement, clipboard, and text-system behavior stays
on the semantic textarea unless Continuity has an explicit structural binding.
Thus Ctrl+Shift+Left/Right remains native word extension, while
Ctrl+Shift+Up/Down retains Continuity's move-line-block behavior. AltGraph and
composition input are never treated as command shortcuts.

The policy follows the browser event boundary: canceling a delivered
`keydown` suppresses its default input action, but user agents define which
browser commands reach page content. Chrome's documented browser accelerators
therefore remain reserved by the default policy. The Electron example uses
`webContents` `before-input-event` only while the editor is focused, cancels
the conflicting menu/page accelerator, then sends the resolved command through
the context-isolated preload. See the [Chrome shortcut list](https://support.google.com/chrome/answer/157179),
[Chrome DevTools shortcuts](https://developer.chrome.com/docs/devtools/shortcuts),
[W3C UI Events](https://www.w3.org/TR/uievents/#event-type-keydown), and
[Electron keyboard shortcut guidance](https://www.electronjs.org/docs/latest/tutorial/keyboard-shortcuts/).

Each accepted mutation emits `continuity-change { version: 1, sequence,
source, commitOrigin, change, snapshot }`. Sequence order is component-local
and monotonic. `commitOrigin="host"` acknowledges a replacement and is never a
persistence request; `user` is the host save boundary.
It is an in-memory acceptance signal, not a durability acknowledgement. A host
persists these events in order and may acknowledge them according to its own
storage contract. The component creates no database, file, worker, Electron
API, or persistence queue.

The component schedules projection work on animation frames, splices line DOM
and matching per-line projection metadata for newline edits, updates active and
edited source-marker lines immediately and preserves valid WYSIWYG projection
on untouched lines. It skips idle parsing while the only dirty line remains
active, requests only the measured source-line window when it becomes
inactive, and reserves complete reconciliation for structural Markdown edits.
The WASM adapter
reuses revision-stamped parse trees and serialized reports; the DOM lane uses
compact presentation, canonical source placeholders for full-document wrap
geometry, and WYSIWYG realization selected from measured cumulative line
offsets for the pixel viewport plus two viewports of overscan. Bulk edits
refresh the dirty range in the fast frame. The source placeholders must not use
`content-visibility`: intrinsic one-row estimates diverge from the semantic
textarea on wrapped documents and can translate all glyphs out of view.
`ResizeObserver` reruns browser font measurement and uses an
end-caret scroll-extent fast path or old/new-width mirrors to preserve caret
screen y. Browser scroll state stays on the textarea and is mirrored to the
visual projection. Inherited CSS custom properties and `theme="light|dark"`
control theme inputs; `--continuity-font-family`, `--continuity-font-size`, and
`--continuity-line-height` control content typography through the shadow
boundary. `prefers-reduced-motion` forces instant behavior.

The standard host `spellcheck` property and `spellcheck="true|false"` attribute
mirror to the semantic textarea. React and framework-neutral controller
configuration accept `spellcheck?: boolean`; the default remains the browser's
enabled state.

## Browser deployment contract

Bundlers should resolve `@continuity-editor/editor/wasm?url` and pass that URL
to `initialize({ wasm })`. Servers must return the asset as
`Content-Type: application/wasm`; aiohttp hosts on Windows may need to register
that MIME mapping explicitly. A strict Chromium CSP must include
`'wasm-unsafe-eval'` in the effective script directive. That source expression
allows WASM compilation without enabling general JavaScript string evaluation;
a same-origin bundled WASM asset requires no additional connection source.

## Electron reference host

`apps/electron-example` is deliberately outside the npm package. Its renderer
uses the same browser component, while a context-isolated preload exposes only
load, packaged-WASM, ordered-persist, and smoke-result IPC calls. The main
process atomically replaces a JSON document in Electron's user-data directory.
No Electron or Node API enters the editor package, and this example storage is
independent of the native application's SQLite database.

The main process owns a durable sequence distinct from the renderer's
component-local sequence. On load it restores text and revision into the WASM
engine; the packed smoke launches twice against one user-data directory and
executes a main-process Ctrl+E interception plus a normal edit on each launch;
both revision and durable sequence must advance from 2 to 4 across restart.

## React host contract

Install the packed artifact into the React application and import
`ContinuityEditor` from `@continuity-editor/editor/react`. React 18.2 through 19
is an optional peer dependency; importing the framework-neutral root does not
require React, and the adapter declaration does not import React types. Preact
compatibility aliases can therefore consume the adapter without installing
`@types/react`. The adapter:

- seeds `value` and `revision` before asynchronous WASM initialization resumes;
- forwards change, request, frame, ready, destroy, and error custom events;
- updates read-only, shortcut, theme, and Tab policy as element properties or
  attributes;
- ignores React renders that repeat the last host snapshot;
- applies changed host text with the supplied engine revision and reports stale
  replacement through `onRevisionConflict`.

Host persistence revisions are never accepted as engine revisions. A React +
HTTP/WebSocket host should update its local Continuity snapshot synchronously,
then feed the text into a bounded single-flight persistence queue carrying its
own ETag/revision. A resource switch remounts the adapter with a resource key;
pane unmount must not be treated as persistence acknowledgement.

Non-React hosts use `attachContinuityEditor(element, options)` before the
element becomes ready. The returned controller forwards native custom events,
updates callbacks and configuration without listener churn, ignores repeated
host snapshots, and exposes revision-checked `synchronize(value, revision)`.
`dispose()` detaches only controller-owned listeners; the host retains element
and persistence ownership.

## Toolchain and build profile

The target is `wasm32-unknown-unknown`. UUID v7 randomness uses getrandom's
`wasm_js` backend selected by `.cargo/config.toml`. Tree-sitter Markdown's
portable C scanners compile with Clang; `crates/wasm/include/wchar.h` supplies
the small locale-free ASCII compatibility surface absent from the upstream
minimal WASM C headers. `-DNDEBUG` prevents the upstream debug assertion shim
from defining a second assertion symbol.

`release-wasm` is separate from the native Windows `release-small` profile:
size optimization, fat LTO, one codegen unit, abort-on-panic, and stripped
symbols apply only to the WASM artifact. `wasm-bindgen-cli` must exactly match
the resolved `wasm-bindgen` crate; `wasm-check` enforces that relationship.

Required local tools are the Rust WASM target, Clang, Node 20 or newer, npm,
and the matching `wasm-bindgen-cli`. Run:

```powershell
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.121 --locked
cargo xtask wasm-check
```

`wasm-package` only builds `target/wasm-sdk/artifacts/*.tgz`.
`wasm-check` additionally compiles the complete portable closure, runs the
native side of the shared serialized parity fixture, installs the tarball into
a clean generated consumer, executes the WASM side under Node, and enforces
the budgets below. npm's cache and consumer are both under `target/wasm-sdk`,
so validation does not depend on or modify the user's global npm cache.

`browser-check` performs the complete `wasm-check` path, installs the tarball
into a separate clean browser consumer, runs the component contract and
accessibility-tree audit in headless Chromium, then installs the same tarball
beside pinned Electron 43.1.1 and verifies renderer-to-main IPC persistence.
On Linux the Electron lane runs under a fresh D-Bus session and Xvfb, and its
unpacked smoke passes `--no-sandbox` at process startup because the npm-owned
SUID helper cannot have root ownership or mode 4755. Normal example launches
omit the flag. The generated browser and Electron consumers remain under
`target/wasm-sdk` for manual inspection.

Set `CONTINUITY_BROWSER_CPU_PROFILE=1` when running `browser-check` to append a
`CONTINUITY_BROWSER_PROFILE` record containing the hottest accumulated Chrome
CPU frames. Profiling is diagnostic and adds enough overhead that its latency
figures are not release evidence; rerun without the variable for authoritative
gate values.

## Parity corpus

`crates/test_fixtures/fixtures/wasm_engine_parity.json` is the single serialized
fixture consumed by native Rust and the packed JavaScript consumer. It covers:

- multi-cursor selection normalization, text, revisions, and ordered deltas;
- Unicode delete-backward and no-op edit behavior;
- typing coalescence, undo, redo, replacement branches, and alternate redo;
- Markdown block/inline decoration;
- visible/hidden display segments and complete source/display mappings.

Changing these semantics requires updating the fixture and both consumers in
one reviewable change.

## Budgets and recorded baseline

These are Node engine gates for the headless synchronous package, not browser
input-to-frame or Web Component claims:

| Measure | Gate | 2026-07-17 local baseline |
|---|---:|---:|
| Module initialization | <=100 ms | 11.303 ms |
| 1,000 single-character edits | p99 <=4 ms | 0.189 ms |
| Linear-memory growth across those edits | <=16 MiB | 327,680 bytes |
| Optimized module, gzip level 9 | <=700 KiB | 320,615 bytes |
| Optimized module, raw | recorded | 1,095,963 bytes |
| Edited 10,000-line viewport projection | product target p99 <=100 ms; shared-runner CI ceiling <=180 ms | 70.1 ms |
| Installed npm package | <=2 MiB | 1,297,896 bytes |
| Installed JavaScript | <=320 KiB | 313,895 bytes (2026-07-30, after projection chrome: host decorations, indent guides, touch selection handles, shared overlay geometry, serializable history; gate raised 208 -> 264 -> 288 -> 320 KiB across those surfaces) |
| Lazy entry | <=2 KiB | 355 bytes |

The reference run used Node 22.17.0 on Windows x86-64. The packed archive was
343.1 kB after adding portable command dispatch and shortcut policies. The
checked-in test owns the gate values and prints a
`CONTINUITY_WASM_METRICS` JSON record on every run. Browser/Electron latency,
accessibility, IME, rendering, and memory budgets must be established by the
Milestone 8 presentation spike rather than inferred from these numbers.

## Browser presentation baseline and gates

The 2026-07-17 Windows x86-64 spike used headless Chrome 150, a 2,000-line
candidate document, and a 1,500-line component document. The selected hybrid
DOM projection plus semantic textarea initialized the large document in
252.8 ms and scrolled it to the end in 33.0 ms. Across 80 sequential edits,
input-to-visible-frame measured 32.9 ms at both p99 and p99.9. The observed JS
heap was 4,483,861 bytes. Candidate construction measured 19.2 ms for a full
contenteditable DOM, 32.9 ms for canvas plus semantic input, and 37.2 ms for
the hybrid viewport fixture; those construction numbers were decision inputs,
not standalone product budgets.

The browser lane owns separate gates:

| Measure | Gate |
|---|---:|
| 1,500-line component ready | <=2,000 ms |
| large-document end scroll | <=100 ms |
| input to paint-ready frame p99 | product target <=50 ms; shared-browser CI ceiling <=75 ms |
| input to paint-ready frame p99.9 | product target <=80 ms; shared-browser CI ceiling <=120 ms |
| 10,000-line component ready | product target <=2,500 ms; shared-browser CI ceiling <=3,750 ms |
| 10,000-line typing dispatch p99 | product target <=160 ms; shared-browser CI ceiling <=240 ms |
| 10,000-line typing frame p99 | product target <=300 ms; shared-browser CI ceiling <=600 ms |
| 10,000-line smart-newline frame p99 | product target <=250 ms; shared-browser CI ceiling <=375 ms |
| 10,000-line alternating-width wrap p99 | product target <=200 ms; shared-browser CI ceiling <=300 ms |
| warm compact presentation p50 | <=100 ms |
| edited viewport presentation p99 | product target <=100 ms; shared-runner CI ceiling <=180 ms |

CI collects 1,024 sequential input-to-frame samples. Under the checked
nearest-rank percentile calculation, the input-to-frame p99 is the eleventh-slowest sample and
p99.9 is the second-slowest; one isolated hosted-runner scheduler interruption
therefore does not masquerade as a percentile regression, while a sustained
tail still fails the calibrated CI ceilings. The browser-result deadline is 180
seconds so the larger sample set can finish on shared runners. Headless
Chromium disables background and occlusion throttling. If, and only if, a
performance assertion fails, the runner navigates to a fresh document and
collects one new 1,024-sample trial; the unchanged budget must pass on one
complete trial. Behavior, accessibility, and integration failures are never
retried.
The component updates its fast projection and emits `continuity-frame` inside
the animation-frame callback. `paintedAt` is the browser-observable paint-ready
boundary after DOM mutation and immediately before Chromium paints when
JavaScript yields; it does not claim access to a compositor presentation
timestamp. This avoids charging a later task or refresh interval to the edit.
The fast frame maintains its source-line mirror by applying the engine's
ordered UTF-16 splices; ordinary edits do not split the complete document or
remeasure the viewport. UTF-8/UTF-16 conversion walks code units directly and
does not allocate a `TextEncoder` buffer per scalar. Packed Unicode coordinate
tests cover line starts, scalar boundaries, and round trips.

The contract currently has 128 behavioral assertions plus the separate latency
gates above, and uses Chromium's
accessibility domain to require one named, multiline, read-only textbox in the
platform accessibility tree with the keyboard-escape description. It also uses
real CDP mouse and keyboard input to verify exact proportional-font pointer
mapping on a projected continuation row, double-click word selection,
triple-click source-line selection, native drag selection,
physical Enter indentation/list/task continuation, physical Shift+Enter raw
newline, Tab/Shift+Tab editing, Ctrl+Shift+Left native word selection, editor-first
Ctrl+R interception, Ctrl+E content-relative task caret placement, exact
Ctrl+Alt+Down and Ctrl+click caret placement,
new-caret activation, wrapped-row vertical movement,
multi-caret typing, and forward/backward Escape-then-Tab traversal at device
scale 1.25. Component assertions also cover smart newline indentation and
list/task continuation, wrapped secondary-caret alignment with a visible
scrollbar, multi-caret Escape, clickable tasks, hanging wraps, heading
hierarchy, and inline/fenced code-copy controls. Long-document regressions require typing
and undo to follow the
caret at both viewport edges, backward-selection measurement to use the active
head, host replacement to preserve scroll, and the visual projection to match
the textarea immediately. A real CDP text insertion repeats the bottom-edge
check outside synthetic event dispatch; the Electron smoke covers both edges
and injects a physical Ctrl/Cmd+Alt+Down chord through `webContents`.
Manual screen-reader acceptance remains a human gate and is recorded in the
presentation-spike evidence document.

## CI

`browser-check` drives two passes over the same page. The main pass runs as a
desktop pointer; a second pass re-runs the touch-shield contract under
coarse-pointer emulation (`Emulation.setEmulatedMedia` pointer/any-pointer
`coarse`, plus touch emulation and mobile device metrics). Without the second
pass the shield is inert by design and none of the touch surface is exercised —
which is how a class of touch defects reached a device unnoticed. The audit
fails the build if it records zero assertions, so a silently skipped pass cannot
read as success.

Synthetic pointer events are not sufficient for touch contracts. A programmatic
`.click()` skips `pointerdown` and fires on elements hit-testing could never
reach (including `display: none`), and a synthetic `pointermove` never triggers
real scrolling. Touch assertions therefore check the `touchmove` default and the
visibility of a control across its own press, not just resulting offsets. The
coarse pass also records semantic-input focus events across pointerdown,
resolved tap, travel with a defensive trailing click, and pointercancel. It
cannot observe physical keyboard visibility/flash, native inertial scrolling,
or Android/iOS user-activation behavior; those remain the device-playground
acceptance matrix in `packages/editor/tests/playground/README.md`.

The `wasm-sdk` Linux job in `.github/workflows/ci.yml` installs the WASM target,
Node 22, D-Bus/Xvfb display tools, and the matching binding CLI, then runs
`cargo xtask browser-check`. The existing Windows-wide CI and native
release/performance profiles remain unchanged. Generated consumer replacement
retries bounded Windows sharing/access violations caused by recently released
Chrome, Electron, antivirus, or indexing handles. A persistent failure names
the exact generated directory; cleanup never targets paths outside
`target/wasm-sdk`.

The separate `sdk-release.yml` workflow first requires an Ubuntu
`browser-check` performance preflight. Its Windows stage then calls the
package-only path once, combines that tarball with native SDK artifacts, and
publishes the `.tgz` through the protected npm environment under `next`.
Publishing jobs never rebuild artifacts.
