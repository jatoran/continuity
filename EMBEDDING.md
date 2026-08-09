# Embedding Continuity

Use this guide to choose and integrate a Continuity editor surface. It is the
public entry point for humans and coding agents; surface-specific READMEs and
design documents remain authoritative for detailed APIs.

## Current coordinates

| Release train | Current version | Canonical source |
|---|---:|---|
| Native Windows desktop | `0.4.2` | `crates/app/Cargo.toml` |
| Embeddable SDK family | `0.2.26` | `sdk/release.toml` |

The release trains are independent. All artifacts in one SDK release use the
same SDK version. Registry publication is not active yet; commands in this
guide consume locally built artifacts unless explicitly marked otherwise.

## Vocabulary

- **Embeddable**: designed to run inside a host application.
- **Headless**: exposes editor state and operations without a visual surface.
- **Visual surface**: owns presentation and user input, but not host storage.
- **Binding**: language transport over the shared engine.
- **Adapter**: platform or framework glue around a shared surface.
- **Host**: application that owns lifecycle, persistence, files, and policy.

Use **Continuity for Windows** for the native Win32 desktop product. The current
cross-platform product name is **Continuity Web**; qualify it as the Electron
desktop application when context is ambiguous. Do not describe macOS or Linux
as native visual targets: their current desktop surface is Electron. The Rust
engine itself is portable.

## Choose a surface

| Host | Use | Visual | Distribution |
|---|---|---:|---|
| Browser, Electron, or webview | `<continuity-editor>` | Yes | `@continuity-editor/editor` |
| React | `ContinuityEditor` adapter | Yes | `@continuity-editor/editor/react` |
| Svelte | Svelte action | Yes | `@continuity-editor/editor/svelte` |
| Vue 3 | Vue composable | Yes | `@continuity-editor/editor/vue` |
| Preact, vanilla, or another web framework | Web Component or neutral controller | Yes | `@continuity-editor/editor/controller` |
| Read-only browser preview | `<continuity-renderer>` | Yes | `@continuity-editor/editor` |
| JavaScript without UI | `Editor` | No | `@continuity-editor/editor` |
| Native Rust application | `continuity_engine::Engine` | No | `continuity-engine` crate |
| Native Win32 Rust host | `continuity_ui::EditorControl` | Yes | Workspace source API; not registry-published |
| Python application | `continuity_editor.Editor` | No | `continuity-editor` wheel |
| C, C++, or FFI-capable native host | `continuity_engine` C ABI | No | DLL plus `continuity_engine.h` |

Python, C, and Rust headless bindings do not provide Qt, Tk, GTK, or Cocoa
widgets. A GUI host in those ecosystems can place the Web Component in a
webview. The Win32 `EditorControl` is the exception: it is a native child HWND.

## Shared architecture

```text
text -> buffer -> engine -------------------------- canonical editing
                    |
                    +-> host ---------------------- intents/events/commands
                    +-> decorate -> display_map --- Markdown projection
                              |
              +---------------+----------------+
              |                                |
      native Win32 adapters             WASM + web adapters
      DWrite/D2D, HWND, UIA              textarea, DOM, CSS, browser AT
```

The following behavior should be implemented once in shared Rust:

- canonical text, selections, revisions, undo, and multi-cursor edits;
- indentation, smart newline, list/task continuation, and portable commands;
- Markdown parsing, decoration, and source/display mapping;
- platform-neutral input intents and revisioned change batches.

The following behavior belongs to an adapter or host:

- DirectWrite/Direct2D versus browser DOM/CSS rendering;
- OS/browser keyboard delivery, IME, clipboard, and accessibility bridges;
- pixel measurement, scrolling, focus, and window lifecycle;
- files, persistence, durability acknowledgements, menus, and application UI.

When a feature changes editing semantics, start in `engine`, `host`,
`decorate`, or `display_map`, then add adapter work only where the platform
must deliver or paint it differently. Do not reimplement an edit planner in
JavaScript, Python, C, Win32, React, Svelte, or Vue. The shared parity corpus
must agree across bindings.

## How the WASM package works

`@continuity-editor/editor` contains one Rust engine compiled to WebAssembly,
the stable JavaScript/TypeScript facade, the Web Component, static renderer,
and framework adapters. React, Svelte, Vue, Preact, vanilla browser, and
Electron integrations all execute the same `.wasm` artifact. They are not
separate engines or separate npm packages.

WASM supplies editor state, operations, Markdown decoration, and display-map
reports. JavaScript supplies the semantic textarea, DOM projection, browser
input, accessibility, scheduling, and host events. The package creates no
database, filesystem, persistence worker, or Electron bridge.

Native Rust, C, Python, and Win32-control integrations do not use WASM; they
compile or bind the native engine.

## Web and Electron integration

Build and validate the exact local npm artifact:

```powershell
cargo xtask browser-check
$artifact = Get-ChildItem target/wasm-sdk/artifacts/continuity-editor-*.tgz |
  Sort-Object LastWriteTime | Select-Object -Last 1
npm install $artifact.FullName
```

For Vite-compatible bundlers, pass the exported WASM URL explicitly:

```js
import { initialize } from "@continuity-editor/editor";
import wasmUrl from "@continuity-editor/editor/wasm?url";

await initialize({ wasm: wasmUrl });
```

Then use the Web Component directly or a framework adapter. The full browser,
controlled-value, React, deployment, shortcut, theming, and headless examples
live in [`packages/editor/README.md`](packages/editor/README.md).

SDK `0.2.26` fixes host-driven reveal navigation without changing the public API.
`revealRange(range, { align })` now waits for the Markdown projection, measures the actual rendered caret or range, and scrolls whichever element owns the viewport.
`nearest` keeps a fitting target fully visible with rendered-row clearance; `center` centers it as closely as scroll bounds permit.
The same projected navigation primitive backs `setSelections(..., { reveal: true })`.
Normal editing also follows the post-render projected caret, preventing typing from moving outside either the desktop textarea viewport or the coarse-pointer touch-shield viewport.
Desktop textarea scrolling and coarse-pointer touch-shield scrolling now agree, including late matches within heavily wrapped source lines.
The operations change selection and viewport only, not text, revision, or undo history.

SDK `0.2.21` stops a touch selection from raising the Android soft keyboard, and
changes no public API. Keyboard visibility on Android is not a function of DOM
focus: the system back gesture hides the IME without blurring, so the editor's
textarea is still focused afterwards and Chrome re-raises the keyboard for any
touch resolving against it — including a long-press that only meant to select.
Focus never moves, so the `0.2.18` focus policy could not reach it. The editor
now holds its internal textarea at `inputmode="none"` on a coarse pointer and
lifts it only where a touch has already resolved as typing: a completed tap, or
an explicit insert (`insertText()` and the built-in paste action). A long-press
claim and an adjust-handle grab restore it, so a gesture that begins as selection
cannot end in a keyboard. Two consequences for hosts. Desktop is untouched — the
gate applies only under `pointer: coarse`, and `inputmode` means nothing to a
physical keyboard. And a host that manages the soft keyboard itself can read
`inputmode="none"` off the textarea as "this editor raises no keyboard right
now", which is a direct answer where focus alone is not one. `focus()` remains a
pure focus move and does not lift the gate; reach the keyboard through a tap or
an insert.

SDK `0.2.20` adds one read-only primitive and changes nothing else.
`visibleLineRange()` returns the inclusive source-line window currently on
screen as `{ startLine, endLine }`, or `null` before first layout and while the
host keeps the editor hidden; `continuity-viewport` publishes the same window
as `{ version: 1, firstLine, lastLine }` whenever it moves. It is the primitive
under scroll-linked host chrome such as a sticky heading trail, and follows the
same split as `setDecorations`: the host owns the model, the editor owns the
geometry. Pixels cannot be converted to lines outside the projection, which
measures headings at up to 1.45em, applies a measured pixel hanging indent to
wrapped rows, and switches its scroller between the textarea and the touch
shield on a live media query. The window is the *visible* one, not the two
extra viewports the projection realizes ahead of the reader; partial lines
count at both edges; a wrapped line reports its own source line; publication is
coalesced to one animation frame and suppressed when unchanged; and one is
emitted after `continuity-ready` to seed a host. The editor does not paint a
heading trail itself.

SDK `0.2.19` fixes two Android-only touch defects and changes no public API.
Enter now commits an open IME composition before running
`editor.insert_newline_smart`, so a list marker is continued whether or not a
composition is open - which on a phone it almost always is, because predictive
keyboards hold one on the word under the caret. Previously the `beforeinput`
router returned before its line-break entries whenever a composition was open,
without preventing the default, so the textarea inserted a raw newline and the
split lost its marker. `Shift+Enter` still inserts a raw newline, and an IME
candidate-commit Enter is still left to the IME, because it raises no line-break
`beforeinput`. The touch selection action bar is now clamped into the visible
text area while any part of the selection is on screen, so a selection taller
than the viewport no longer carries the clipboard actions off screen with its
start; the clamp releases once the selection is entirely off screen, and a
selection that fits keeps the placement it had.

SDK `0.2.18` changes the Web Component's default touch-focus arbitration.
Touch `pointerdown` records the gesture without focusing the textarea or
interrupting native single-finger scrolling. A resolved tap commits the
projected caret and synchronously focuses the textarea during the trusted click
turn, preserving mobile browser keyboard activation. Pan, cancellation, and
long-press paths remain unfocused; mouse and pen behavior is unchanged. Hosts
do not need to inspect the shadow DOM or reproduce projected Markdown
hit-testing. The preceding SDK `0.2.17` adds projection chrome and fixes two
Markdown rendering defects.
A thematic break (`---`) now draws a rule instead of projecting to a blank line,
suppressed on the caret's own line the way heading sizing is. A tab-indented
line's wrapped rows now hang beneath its own content: the hanging indent had
been expressed as inline padding plus a negative first-line indent, and CSS
anchors tab stops at the *content* edge, so the padding shifted the whole tab
grid right by the indent while the negative indent pulled the first row left by
it — a nested bullet rendered its text at `indent mod tab-width` while its
wrapped rows hung at the full indent, by an amount that changed with the font
and never affected space-indented lines. `text-indent: <width> hanging` needs no
padding, so the grid origin stays under the first row. Unrealized lines are now
measured against the projection's font metrics rather than `ch` units, so a line
does not shift when it scrolls into the realized window. Three additions:
`indent-guides="on"` paints vertical rules at each enclosing indent level
(off by default, themed through `--continuity-indent-guide` and
`--continuity-indent-guide-active`, with the desktop painter's column
semantics); `setDecorations(id, ranges)` / `clearDecorations(id?)` paint host
ranges without touching selection, history, or revision — themed per set through
`--continuity-decoration-<id>` and exposed as `part="decoration decoration-<id>"`
— which is what a host-side find bar needs, because only the measured viewport
is in the DOM for a browser find to reach; and touch selections gain adjust
handles, since the shield displaces the platform's own and a selection that
cannot be nudged has to be redrawn from scratch. `exportHistory()` /
`importHistory()` move the undo tree across an unmount as plain JSON, checksum-
guarded against a different document, for hosts that unmount per tab. The
preceding
SDK `0.2.16` opens the command rail to the host. `registerRailAction(action)`
(disposer returned) and the `railActions` property/React prop add host buttons
alongside the built-ins: an action either names a storage-neutral engine
command, runs a host callback with the element, or names neither and raises
`continuity-request` with `kind: "railAction"` for declarative hosts. Ids are
namespaced `vendor:action` because the id is the persistence key — and a stored
arrangement now retains ids whose action is not registered yet, so a host that
registers late returns to the slot the user left it in instead of the rail's
tail. Icons are host-built element factories rather than markup strings,
`isEnabled(snapshot)` disables a button in place on every commit and selection
change, each button exposes `part="command-rail-button command-rail-button-<id>"`,
and `rail-storage-key` scopes the arrangement so one origin can run several
rails. Four built-ins ship with it: `move-line-up` / `move-line-down` move every
source line the selection covers as one block, and `caret-up` / `caret-down`
move the caret one *visible* row, walking a soft-wrapped line's rows the way the
platform arrow keys do. Those two are the only rail actions with no engine
command — wrapped-row geometry is browser-side — so a host cannot reimplement
them from outside. The preceding
SDK `0.2.15` completes the touch clipboard. A selection action bar (Copy, Cut,
Paste, Select all) replaces the platform bubble the touch shield displaces, and
a long-press offers it even with nothing selected — Copy and Cut hide
themselves rather than sitting inert, so a bare caret still reaches Select all
and Paste. A selection drag held at the top or bottom edge auto-scrolls and
keeps extending. Paste degrades instead of dying: `clipboard.readText()` first,
then `execCommand("paste")` for embedders that still honour it, then the text
this editor last copied or cut, and finally a `pasteText` host request. That
matters because an embedding frame which does not delegate `clipboard-read`
denies the read outright and no page-side code can lift it — in-editor
copy/paste keeps working there, only cross-application paste needs the frame to
add `allow="clipboard-read"`. A new public `insertText(text)` is how a host
answers `pasteText`. The preceding
SDK `0.2.14` moves the touch surface off the textarea. On a coarse pointer a
transparent shield covers the editing surface, takes the finger, and owns the
scrolling, while the textarea keeps focus, the soft keyboard, IME, and the
document value beneath it. This is forced rather than preferred: the platform's
long-press selection hit-tests the invisible textarea, whose layout cannot match
a projection that folds Markdown markers, scales headings, and is narrower — and
on an editable element nothing refuses that gesture (`selectstart` is not raised,
`contextmenu` arrives after the selection already happened, `user-select` does
not apply). On a plain div they do. A mouse still addresses the textarea
directly, so fine-pointer behaviour is unchanged. Long-press now selects the
word under the finger through the projection and drags extend through projected
glyphs, anchored on the whole word; `contextMenu` requests carry
`isLongPressSelection`. The cost on touch is the platform magnifier, selection
handles, and paste bubble. The shield's scrollable extent is the projection's
height by construction, which fixes documents whose tail could not be scrolled
to, and scrolling layers are inset above the command rail so no rows hide behind
it. Platform caret moves that raise no `select` event are adopted through
`selectionchange`, and taps taken during an open composition commit the
composing run and apply the projected position instead of the platform's.
The preceding
SDK `0.2.13` fixes three Markdown-editing defects. The command rail's checkmark
now creates a full Markdown task (`- [ ] `) via `markdown.toggle_task` instead of
a bare inline `[ ] ` prefix (its rail id stays `checkbox`, so saved arrangements
survive; the separate `markdown.toggle_checkbox` still backs clicks on a rendered
task). The rail's bullet and task actions now keep the caret on the same content
character when toggling a marker, sharing the content-relative line-start planner.
And a pointer tap taken while an IME composition is active is hit-tested against
the live composing line and applied only after `compositionend` reconciles, so an
Android predictive-keyboard tap lands on the character under the finger instead of
one byte off until the next space — with no Android/Gboard/user-agent branch.
The preceding
SDK `0.2.12` fixes touch selection: a finger is no longer treated as a mouse.
Touch pointers keep their native contract (drag pans, long-press opens the
platform selection handles, tap focuses), mouse-style drag selection and
pointer capture are mouse/pen-only, a canceled touch drops all gesture state,
and taps use a finger-sized slop so a jittery tap still places the caret
through the projected glyph under the finger instead of falling back to
raw-layout placement. It also preserves the scroll viewport across a host
`display: none` hide/show cycle, so tab-style hosts that keep the editor
mounted no longer snap to the top on tab return (hosts that unmount per tab
still use `getScrollState()`/`restoreScrollState()`). The preceding
`0.2.11` makes rail and copy-control chrome sizing independent of the
embedding page's root font-size: dimensions are px-based with touch-first
defaults (48px rail buttons in a 56px rail, 44px settings controls), and
hosts retune them via `--continuity-rail-height` (rail and content inset move
together), `--continuity-rail-button-size`, and `--continuity-rail-font-size`.
Previously a dense host root (11px) shrank every `rem`-sized control well
below tappable size. The preceding
`0.2.10` adds mobile chrome: a bottom quick-action command rail
(`command-rail="auto|on|off"`, automatic on touch-primary devices) that
executes editor-owned engine commands without dismissing the virtual keyboard
and whose button set is enable/disable/reorderable through a built-in settings
panel persisted in `localStorage`; and touch code-copy fixes — the
fenced-block Copy control is permanently visible where there is no hover, and
copying falls back from the async Clipboard API to a selection-based copy
before requesting host mediation. The preceding
`0.2.9` scopes IME composition to the composing line: the rendered
Markdown projection stays visible for the whole document while a mobile
keyboard composes each word, the composing line previews the live textarea
text with the composed run underlined, and only an unmappable composed run
(for example one containing a newline) falls back to a frame-wide native
reveal. This removes the per-word whole-document raw-source flash on Android
and iOS keyboards. The preceding
`0.2.8` makes full-document reconciliation a minimal splice and the live
textarea caret authoritative after composition commits and autocorrect-style
native mutations, so mid-document mobile typing no longer relocates the caret
to the document end, native replacements cannot duplicate text, and the idle
projection pass reconciles per line instead of repainting the whole note. The
preceding `0.2.7` patch keeps the semantic selection in the textarea but paints
carets and non-collapsed ranges from measured projection glyphs. Simple click,
Shift+click, and primary-button drag use that same projection mapping,
including on wrapped Markdown lines and host-selected fonts. Selection endpoint
lines reveal source; intermediate lines stay rendered, and highlight work is
viewport-bounded. Native selected text remains transparent so it
cannot overpaint the projection, and browser-owned navigation keys repaint the
projected caret after their default movement without requiring a subsequent
edit. Projected double-click selects a word and triple-click selects the source
line through the same shared Rust semantics as the native editor. The `0.2.6`
patch hardens the mobile IME path: host writes defer during composition and the
composed run no longer doubles on commit (see the Web Component section in the
package README).

Deployment requirements:

- serve `.wasm` as `Content-Type: application/wasm`;
- strict Chromium CSP permits `'wasm-unsafe-eval'` in `script-src` or the
  effective `default-src`;
- initialize once per JavaScript agent and share the memoized result;
- use `shortcutPolicy="browser-safe"` in a normal browser and consider
  `editor-first` only in a controlled shell such as Electron.

## Controlled state and persistence

Continuity revisions and host database/file revisions are different values.
Keep them separate.

Every accepted mutation returns or emits an in-memory snapshot. That is not a
durability acknowledgement. The host must persist accepted user changes in
order and decide when storage is durable.

For the Web Component and its adapters:

- persist `continuity-change` only when `commitOrigin === "user"`;
- do not persist `commitOrigin === "host"`; it acknowledges host replacement;
- synchronize complete `{ text, revision }` engine snapshots;
- handle `RevisionConflictError` instead of overwriting newer typing;
- flush or explicitly abandon the host save queue during teardown.

During an active IME composition the component defers every host document
replacement so the internal textarea is never rewritten mid-composition; on
mobile predictive keyboards (Android GBoard, iOS Safari) rewriting it would make
the keyboard re-inject and compound the document on the space key. `replaceValue`
returns `null` while composing (the write is queued) and the queued write is
applied through the revision guard at `compositionend`, so a stale controlled
echo no-ops rather than duplicating the just-committed word. The commit itself
reconciles as a minimal splice and adopts the live textarea caret, so typing
mid-document never relocates the caret or repaints untouched lines. The
composition presentation itself is line-scoped: the rendered Markdown
projection stays visible for the whole document and only the composing source
line previews the live textarea text, so mobile keyboards that compose every
word cannot flash the document between raw source and rendered Markdown while
typing. The element's
`composing` property reports the active-composition state; the supplied
controlled adapters already suppress reconciliation while it is true, so custom
controllers should read it before forcing a `replaceValue`.

The optional `@continuity-editor/editor/commit-queue` helper implements a
single-in-flight, newest-wins debounced queue. It still delegates actual
storage to the host.

## Rust, C, and Python integration

For a local Rust host:

```toml
[dependencies]
continuity-engine = { path = "C:/path/to/continuity/crates/engine" }
```

```rust
let mut engine = continuity_engine::Engine::new();
let document = engine.open_buffer("# Host-owned text\n");
```

Run the packed native-language consumer gate before integration:

```powershell
cargo xtask sdk-check
```

It stages clean Rust archives, the C DLL/header, and the Python wheel under the
newest `target/sdk-check/<run>/` directory. Install the wheel with:

```powershell
$run = Get-ChildItem target/sdk-check -Directory |
  Sort-Object LastWriteTime | Select-Object -Last 1
python -m pip install --force-reinstall (Get-ChildItem "$($run.FullName)/wheels/*.whl")
# uv-managed environment:
uv pip install --reinstall (Get-ChildItem "$($run.FullName)/wheels/*.whl")
```

See [`native-sdk.md`](.docs/technical/native-sdk.md) for allocator, thread,
callback, packaging, and teardown contracts. See
[`embeddable-windows-control.md`](.docs/design/features/embeddable-windows-control.md)
for native child-control ownership and construction.

## Validation

Choose gates that exercise the artifact the host will consume:

| Integration | Required gate |
|---|---|
| npm/Web Component/React/Svelte/Vue | `cargo xtask browser-check` |
| Rust/C/Python | `cargo xtask sdk-check` |
| Native Windows control | `cargo xtask bench-fast` plus workspace CI |
| Electron desktop product | `cargo xtask desktop-check` |
| Coordinated SDK release | `cargo xtask sdk-release-check` then release dry run |

All meaningful changes also run `cargo xtask conventions`, `cargo xtask ci`,
and `cargo xtask docs-check`. Performance budgets are release gates, not
optional observations.

## Detailed references

- Public names, ownership, compatibility, and version trains:
  [SDK contract](.docs/design/sdk-contract.md)
- WASM/npm internals, browser contracts, and budgets:
  [WASM SDK](.docs/technical/wasm-sdk.md)
- Web Component input, rendering, accessibility, and shortcuts:
  [Web Component](.docs/design/features/web-component.md)
- Rust/C/Python artifacts:
  [native-language SDK](.docs/technical/native-sdk.md)
- Native Win32 visual embedding:
  [Windows editor control](.docs/design/features/embeddable-windows-control.md)
- Electron desktop ownership and packaging:
  [cross-platform desktop](.docs/design/features/cross-platform-desktop.md)

## Agent handoff template

Give an integration agent this file plus the host application's architecture,
then use the following instruction:

```text
Integrate Continuity using EMBEDDING.md as the routing contract. Choose the
smallest supported surface for this host. Reuse the shared engine or Web
Component; do not fork editing logic. Keep Continuity engine revisions separate
from host storage revisions. Treat accepted changes as in-memory until the host
durably persists them. Run the packed consumer gate for the selected artifact
and report any integration friction against the owning package README.
```

When adding or changing a public surface, package coordinate, adapter,
supported target, install command, version, deployment requirement, ownership
rule, or persistence contract, update this guide in the same change.
