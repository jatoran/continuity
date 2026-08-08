# @continuity-editor/editor

Accessible `<continuity-editor>` Web Component and synchronous,
storage-neutral Continuity editor engine compiled to WebAssembly. The package
contains the component, TypeScript declarations, styles, source map, optimized
WASM, license, and per-file integrity metadata.

## Browser use

Importing the package defines the component. Initialize the shared WASM module,
set the initial value as a property, and wait for `ready` before reading state:

```js
import { initialize } from "@continuity-editor/editor";

await initialize();
const editor = document.querySelector("continuity-editor");
editor.value = "# Hello\n";
editor.initialRevision = savedRevision;
await editor.ready;

editor.addEventListener("continuity-change", async ({ detail }) => {
  if (detail.commitOrigin === "user") {
    await saveInYourHost(detail.sequence, detail.snapshot);
  }
});
```

```html
<continuity-editor aria-label="Project notes"></continuity-editor>
```

## Deploying

For Vite and compatible bundlers, import the exported WASM asset as a URL and
pass it explicitly. This is the canonical browser deployment path:

```js
import { initialize } from "@continuity-editor/editor";
import wasmUrl from "@continuity-editor/editor/wasm?url";

await initialize({ wasm: wasmUrl });
```

Serve `.wasm` as `Content-Type: application/wasm`; this is required by
`WebAssembly.instantiateStreaming`, and is mandatory when
`X-Content-Type-Options: nosniff` is enabled. Python's Windows MIME table may
not contain the mapping, so an aiohttp host can register it during startup:

```py
import mimetypes

mimetypes.add_type("application/wasm", ".wasm")
```

A strict Chromium Content Security Policy must allow WASM compilation. Add
`'wasm-unsafe-eval'` to `script-src` (or the effective `default-src`), for
example `script-src 'self' 'wasm-unsafe-eval'`. This does not enable general
JavaScript `eval`, and a same-origin bundled asset needs no additional
`connect-src` permission. See the [CSP specification](https://www.w3.org/TR/CSP3/#directive-script-src),
[MDN streaming compilation requirements](https://developer.mozilla.org/docs/WebAssembly/Reference/JavaScript_interface/instantiateStreaming_static),
and [Vite static asset imports](https://vite.dev/guide/assets.html).

## Framework-neutral controlled use

Vue, Svelte, Preact, and vanilla hosts can reuse the same controlled snapshot
semantics through `@continuity-editor/editor/controller`. Attach before the
element becomes ready, synchronize only complete engine snapshots, and dispose
the controller when the host view releases the element:

```js
import { attachContinuityEditor } from "@continuity-editor/editor/controller";

const element = document.createElement("continuity-editor");
const controller = attachContinuityEditor(element, {
  value: savedText,
  revision: savedEngineRevision,
  callbacks: {
    onChange: ({ snapshot }) => persistInHost(snapshot),
  },
});
mount.append(element);

await controller.synchronize(reloadedText, reloadedEngineRevision);
controller.dispose();
```

`synchronize()` uses revision-checked `replaceValue` and rejects with
`RevisionConflictError` rather than overwriting newer typing. `dispose()` only
detaches controller listeners; the host still owns the element. The React
adapter declaration is self-contained, so Preact consumers using
`preact/compat` do not need `@types/react` solely to import it.

Every change has `commitOrigin: "user" | "host"`. Persist only `"user"`;
`"host"` acknowledges a host replacement and must not loop back into storage.
`onHostReplacement` is the dedicated controller/React callback. The controller
also forwards selection, reveal, scroll-state, and linear-memory APIs.

Svelte hosts can use `/svelte`; Vue 3 hosts can use `/vue` (Vue is an optional
peer). `/lazy` dynamically loads route-local editors. `/conformance` exports a
disposable browser wiring harness, and `/commit-queue` provides an optional
single-in-flight, newest-wins debounced save queue with a maximum-delay cap.

## Plain text and static rendering

Set `syntax="plain"` to retain the rope, selection, undo, multi-cursor, and
host contracts while disabling Markdown projection. `<continuity-renderer>`
provides selectable, read-only Markdown or plain projection without an input
textarea.

## React use

The optional `@continuity-editor/editor/react` subpath supplies a controlled
React adapter without adding React to non-React consumers. Initialize WASM once
before mounting the application, then keep the complete Continuity snapshot in
React state:

```tsx
import { useState } from "react";
import { initialize } from "@continuity-editor/editor";
import { ContinuityEditor } from "@continuity-editor/editor/react";

await initialize();

export function NoteEditor({ initialText, persist }) {
  const [snapshot, setSnapshot] = useState({ text: initialText, revision: 0 });
  return (
    <ContinuityEditor
      aria-label="Project note"
      spellcheck={false}
      style={{ display: "block", width: "100%", height: "100%" }}
      value={snapshot.text}
      revision={snapshot.revision}
      onChange={(detail) => {
        setSnapshot(detail.snapshot);
        persist(detail.snapshot.text);
      }}
    />
  );
}
```

`revision` is the Continuity engine revision corresponding exactly to `value`.
It is not a database ETag, file mtime, or host optimistic-concurrency token.
Hosts keep those persistence revisions separately. Unchanged React props never
echo a replacement after local typing; a changed host snapshot is applied with
`replaceValue(value, revision)`, and stale replacement attempts call
`onRevisionConflict` instead of overwriting newer editor state. Use a stable
resource key and remount with revision zero when switching to a different note
unless the host persisted the previous Continuity revision.

The component uses a semantic `<textarea>` for input, IME, canonical selection,
clipboard, keyboard navigation, and accessibility. A DOM projection consumes
the shared Rust display map for Markdown presentation. Primary and secondary
carets and non-collapsed selection highlights are painted from that projection's
measured glyph rectangles, so wrapped rows cannot drift from raw-source
textarea geometry. Both the native selection background and
foreground remain transparent so raw source cannot overpaint projected glyphs.
IME composition is line-scoped: the rendered Markdown projection stays visible
for the whole document while the composing source line previews the live
textarea text (composed run underlined) and the overlay paints the live caret
and same-line selection against that preview. Mobile keyboards compose a word
at a time, so this is what keeps headings, markers, and soft-wrap layout from
flashing across the entire document on every composed word; only a composed
run that cannot be mapped onto the line structure (for example one containing
a newline) falls back to a frame-wide native glyph, caret, and selection
reveal. While a composition is active the internal textarea is
never rewritten programmatically: host `value`/`replaceValue` writes and
controlled reconciliation defer until `compositionend`, then apply through the
revision guard so a stale echo cannot duplicate the just-composed word. This is
what keeps predictive mobile keyboards (Android GBoard, iOS Safari) from
re-injecting and compounding the document on the space key. A composition
commit — and any other textarea-first mutation, such as an autocorrect
`insertReplacementText` — reconciles as a minimal splice and adopts the live
textarea caret into the engine, so the caret never jumps away from the
committed word and distant lines are untouched. A pointer tap taken while a
composition is still active is hit-tested against the live composing line and
replayed against the fresh snapshot only after `compositionend` reconciles, so
the caret lands on the tapped character rather than a stale-mapped byte — the
fix carries no Android/Gboard/user-agent branch. The `composing`
property reports whether a composition is currently active. The semantic
textarea also disables `autocorrect` alongside `autocomplete` and
`autocapitalize` to reduce mobile composition churn. It never creates a
database or writes files: hosts decide whether changes are ephemeral or are
persisted after each ordered `continuity-change` event.

Public element APIs include `value`, `initialRevision`, `readOnly`,
`spellcheck`, `syntax`, `indentGuides`, `composing`, the `command-rail`,
`rail-storage-key`, and `indent-guides` attributes, shortcut configuration,
`snapshot()`, `replaceValue()`, `executeCommand()`, `setSelections()`,
`revealRange()`, `setDecorations()`, `clearDecorations()`, `exportHistory()`,
`importHistory()`, `registerRailAction()`, `railActions`,
scroll capture/restore, `visibleLineRange()`, `linearMemoryBytes()`, `focus()`,
and `destroy()`.

### Revealing source ranges

`revealRange(range, { align: "nearest" | "center" })` makes the range the primary selection and reveals its rendered Markdown geometry after projection.
`nearest` is the default and keeps a fitting target fully visible with one rendered-row edge clearance.
`center` centers the rendered target as closely as scroll bounds permit.
The editor resolves the current scroll owner, including the coarse-pointer touch shield, and accounts for heading scale, folded markers, wrapping, hanging indents, and desktop projection-scroll compensation.

`setSelections(selections, { reveal: true })` uses the same projected navigation primitive for the primary selection head.
Neither operation changes document text, revision, or undo history.
Ordinary edits also use the post-render projected caret, so typing cannot continue below or above the visible viewport when Markdown styling or wrapping diverges from the textarea.
Revision-checked replacement throws `RevisionConflictError` rather than
overwriting newer typing, and returns `null` when it defers because an IME
composition is active. Set `initialRevision` before readiness when restoring
host-managed state; the next edit advances from that revision. Host-owned link activation, context menus, and file
drops arrive as `continuity-request` events. All event payloads carry protocol
version `1`; mutating event streams also carry a monotonically increasing
sequence number.

Spellcheck follows the browser default. Set the standard `spellcheck` property
to `false`, use `<continuity-editor spellcheck="false">`, or pass
`spellcheck={false}` to the React adapter to disable it on the semantic
textarea. Changes apply immediately; shadow-root access is unnecessary.

Set `theme="light"` or `theme="dark"`, or override the inherited
`--continuity-*` custom properties. `--continuity-font-family` and
`--continuity-font-size` control the content font; `--continuity-line-height`
controls its line height. `--continuity-indent-guide` and
`--continuity-indent-guide-active` colour the optional indent guides;
`--continuity-decoration` and `--continuity-decoration-<id>` colour host range
decorations. Tab indents selected lines through the
shared engine and Shift+Tab outdents them. Press Escape, then Tab or Shift+Tab,
to move focus out; the semantic input exposes this instruction to assistive
technology. Read-only editors release Tab directly. A form-like host may set
`tab-behavior="focus"` to make direct focus traversal the default.
Reduced-motion preferences disable component motion. Soft-wrap width changes
preserve the active caret row's screen y while browser typography is
remeasured. Typing, deletion, paste/cut/drop, indentation, composition commit,
undo, and redo reveal the active selection head when it crosses either viewport
edge and synchronize the visual projection in the same mutation turn. Host
`value`/`replaceValue` updates apply the minimal differing splice and preserve
the host-selected viewport; selections outside the changed range keep their
positions.

The visual layer keeps canonical source text as full-document geometry and
realizes WYSIWYG projection for the measured pixel viewport plus two viewports
of overscan. Fast edits invalidate projection metadata only for touched lines:
active and structurally dirty lines reveal source immediately while untouched
lines retain their rendered Markdown during continuous typing. Line-count edits
splice source placeholders and aligned projection metadata in the fast frame,
so soft-wrapped content cannot translate the projection away from the revealed
caret. Ordinary character input does no idle parse work while its source line
remains active. When that line becomes inactive, the 250 ms reconciliation
requests only the measured source-line viewport from Rust. Structurally
significant edits retain a complete block reconciliation.

Shortcut policy defaults to `browser-safe`, which releases Chromium UI and
DevTools conflicts such as Ctrl/Cmd+E, K, R, Shift+R, J, Shift+J, U, and
Shift+C. Use `shortcut-policy="editor-first"` in a controlled Electron shell,
or `shortcut-policy="none"` when the host owns every command chord. Set
`shortcutBindings` to a record or map such as `{ "Mod+E":
"markdown.toggle_task", "Mod+R": null }`; explicit entries override the
selected policy. A browser may withhold browser-UI accelerators before the page
receives them, so hosts can call `executeCommand()` directly or mediate them in
Electron's main process. Native textarea motion remains intact: for example,
Ctrl+Shift+Left/Right extends by word.

The built-in non-browser-safe defaults are Mod+J, Mod+Shift+J, Mod+R,
Mod+Shift+R, Mod+U, Mod+Shift+S, Mod+E, Mod+Shift+C, and Mod+K. Call
`listShortcutBindings()` to inspect the complete binding table and its
`isBrowserSafe` flags. When a delivered built-in chord is released by policy,
the element emits `continuity-shortcut-suppressed` with `{ chord, command,
policy }`. To reclaim one known conflict without switching to `editor-first`,
set an explicit override such as `{ "Mod+E": "markdown.toggle_task" }`.

Physical Enter is claimed before Chromium's textarea default, preserves leading
indentation, and continues bullets, ordered lists, and task checkboxes through
the shared smart-newline planner; Shift+Enter inserts a raw newline. A
soft-keyboard Enter arrives inside an IME composition, where `keydown` carries
no usable key, so the line-break `beforeinput` commits the composing run and
runs the same planner. Marker continuation, ordered numbering, task items
restarting as `- [ ] `, empty-item outdent, and nested indentation are therefore
identical with a composition open or closed. An IME candidate-commit Enter
raises no line-break `beforeinput` and is still left to the IME.
Ctrl/Cmd+Alt+Up/Down and Ctrl/Cmd+click outside Markdown links add
carets and keep the newest caret active. Vertical additions follow wrapped
visual rows, edits apply to every caret, and Escape clears secondary carets before
arming focus traversal. Multi-range copy/cut joins ranges with newlines.
Simple clicks are hit-tested against the rendered projection's actual grapheme
rectangles and mapped back through display-map segments to canonical source.
This keeps wrapped-line placement exact under host-provided proportional fonts,
font sizes, browser zoom, and device scale. Shift+click and primary-button drag
use the same projection mapping. Double-click selects the shared Rust word at
that canonical source position; triple-click selects the current source line,
excluding its line ending. Keyboard selection remains a semantic textarea
operation. Browser-owned Arrow/Home/End/Page movement reconciles the projected
caret after the browser default on every repeated keydown, without waiting for
an edit. Selection endpoints reveal source; intervening lines stay projected,
and only the visible projection window receives highlight rectangles.
Task toggling preserves content-relative
selection endpoints, and a fresh-line Ctrl/Cmd+E lands after `- [ ] `.
Wrapped indented/list lines use a measured pixel hanging indent, so every
continuation row starts exactly beneath the first row's content rather than a
font-dependent character estimate. The indent is applied as
`text-indent: <width> hanging` rather than as inline padding plus a negative
first-line indent: CSS anchors tab stops at the block's *content* edge, so
padding shifts the whole tab grid right by the indent while the negative indent
pulls the first row left by it, and a tab-indented line's own content then
renders left of the rows hanging beneath it. Lines outside the realized
viewport window are measured against the projection's font metrics rather than
`ch` units, so a line does not shift as it scrolls into that window. Thematic
breaks (`---`) draw a rule; like heading sizing it steps aside on the caret's
own line, which shows raw source. An overwide unbroken first token begins
beside the list marker and breaks across following rows instead of stranding
the marker on a row by itself. Inactive headings have a level-specific size
hierarchy, task markers are clickable, and hovering inline or fenced code
reveals a Copy control. On touch-primary devices (`pointer: coarse`) the
fenced-block Copy control is always visible — touch has no hover — and inline
controls stay tap-revealed. Copying tries the async Clipboard API, then a
selection-based fallback that also works in insecure LAN contexts; only if
both are rejected does the component emit `continuity-request` with
`kind: "copyText"` for host mediation.

On touch-primary devices the editor also shows a bottom quick-action command
rail (set `command-rail="on"` or `"off"` to override the automatic default).
The rail executes editor-owned engine commands — undo, redo, indent, outdent,
bullet, task, bold, italic, inline code, heading cycle, and more — without
dismissing the virtual keyboard. The checkmark action creates a full
Markdown task (`- [ ] `); its rail id stays `checkbox` so saved arrangements
survive. The bullet and task actions keep the caret over the same character
when toggling a marker on or off. `move-line-up` / `move-line-down` move every
source line the selection covers as one block (one undo group, ordered-list
markers renumbered), and `caret-up` / `caret-down` walk one *visible* row, so a
caret inside a soft-wrapped line moves within that line the way the platform
arrow keys do rather than jumping a whole paragraph. The gear button opens a
settings panel to enable, disable, and reorder actions. The arrangement
persists in `localStorage` under `continuity-editor.command-rail`, scoped by
`rail-storage-key="<name>"` when one origin runs several distinct rails. The
rail hides while the editor is read-only, and `part="command-rail"` /
`part="command-rail-settings"` /
`part="command-rail-button command-rail-button-<id>"` allow host styling. Rail and copy-control
chrome is sized in `px` — not `rem` — so a host page with a dense root
font-size cannot shrink touch targets; defaults are 48px buttons in a 56px
rail. Hosts retune with `--continuity-rail-height` (rail plus content inset
together), `--continuity-rail-button-size`, and
`--continuity-rail-font-size`.

Hosts extend the rail with their own actions. `registerRailAction(action)`
returns a disposer; assigning `railActions` (or passing the `railActions` prop
to the React adapter or controller) replaces the whole host set at once.
Built-in actions are never affected. `listBuiltInRailActions()` enumerates the
ids a host can reorder or switch off.

```js
const dispose = editor.registerRailAction({
  id: "acme:summarize",              // namespaced; bare ids are reserved
  label: "Summarize note",           // required, becomes the button aria-label
  glyph: "∑",                        // or icon: () => svgElement
  run: (element) => summarize(element.snapshot().text),
  isEnabled: (snapshot) => snapshot.text.length > 0,
  isEnabledByDefault: true,
});
```

An action names exactly one behavior, resolved in this order: `command` runs a
storage-neutral engine command through the same resolver as the keyboard
shortcuts, `run` calls back with the element, and an action with neither raises
`continuity-request` with `kind: "railAction"` and the `actionId` — the route
for a declarative host that cannot pass a callback. `icon` is a factory that
returns an element the host builds itself; the editor never accepts markup
strings into its shadow root. `isEnabled` is re-evaluated on every commit and
selection change, and a failing predicate disables the button in place rather
than removing it, so the rail never shifts under a finger already on its way
down. A throwing `run` or `icon` surfaces as `continuity-error` and leaves the
rail intact.

In React, memoize the `railActions` array (or register once in an effect and
keep the disposer): a new array identity rebuilds the rail's buttons, the same
way any object-valued prop does.

Ids are the persistence key: keep them stable. A stored arrangement that names
an action which is not registered yet retains that id and its slot, so a host
that registers late (after `ready`, behind a feature flag, or on a route
change) returns to the position the user left it in instead of the rail's tail.

Touch selection is projection-owned, and on a coarse pointer the finger never
lands on the textarea at all. A transparent shield (`.touch-shield`) covers the
editing surface, takes the touch, and owns the scrolling; the textarea keeps
the semantic focus, soft-keyboard, IME, and document roles beneath it with
`pointer-events: none`. Touch `pointerdown` only records the gesture: it does
not focus, capture, or prevent native scrolling. A resolved tap first commits
the caret through measured projection geometry and then synchronously focuses
the textarea in the same trusted click turn, so the soft keyboard opens on the
tap while a pan or cancellation never opens or briefly flashes it. This is the
component default; hosts do not inspect the shadow DOM or reproduce caret
hit-testing. The frame carries `touch-scrolling` while this is active, and the
same predicate picks the scroller in JavaScript, so the stylesheet and the code
cannot disagree.

This is forced, not preferred. The platform's long-press selection hit-tests the
invisible textarea, whose layout cannot be made to match the projection — folded
Markdown markers shorten projected text, headings render at up to 1.45em, the
projection box is 14px narrower, and a textarea has no per-line font-size to
reconcile them with. Nothing refuses that gesture on an editable element:
`selectstart` is not raised for it, `contextmenu` arrives *after* the selection
has already happened, and `user-select` does not apply. On a plain non-editable
div all three work, so the shield is the only place the editor can win. A mouse
still addresses the textarea directly, so pointer behaviour on a fine pointer is
unchanged.

With the finger on the shield, a long-press selects the word under it through
the projection and the drag extends through projected glyphs. The anchor is the
whole word rather than one of its ends, so a finger resting inside it keeps it
selected and a drag grows from whichever edge it passes. The `continuity-request`
event for `contextMenu` still fires and carries `isLongPressSelection`; mouse
right-click keeps the platform context menu. The cost is the platform magnifier,
selection handles, and long-press paste bubble on touch.

While a selection drag rests near the top or bottom edge the surface
auto-scrolls under it, at a rate that grows with how far into the edge band the
finger sits, and the selection keeps extending as content moves. This is needed
because the drag refuses the scroll (see above), so the finger cannot otherwise
reach text beyond the viewport.

Taking the platform's selection away also removes the bubble it carried, so the
editor supplies its own: a Copy / Cut / Paste / Select all bar appears once a
selection settles and the finger lifts, positioned clear of the highlight and
following it on scroll. A long-press offers the bar even with nothing selected -
Copy and Cut hide themselves rather than sitting inert, so a bare caret still
reaches Select all and Paste. A plain tap never summons it. A selection can be
taller than the viewport, so the bar is clamped into the visible text area while
any part of the selection is on screen and pins to the edge it would otherwise
scroll past; the clamp releases once the selection is entirely off screen, and a
selection that fits keeps the placement it had.

Paste degrades rather than dying. In order: `clipboard.readText()`;
`execCommand("paste")`, which Chrome refuses but Electron and several WebViews
honour; the text this editor last copied or cut; and finally a
`continuity-request` of kind `pasteText`. The fallback to the editor's own last
copy is what keeps paste alive where a permissions policy denies
`clipboard-read` — an embedding frame that did not delegate it is the usual
cause, and a document cannot grant itself what its parent withheld, so no
page-side code can lift it. Only cross-application paste needs the frame to add
`allow="clipboard-read"`. Copy failures fall back through the textarea's own
selection and then a `copyText` request.

`insertText(text)` is the public method a host uses to answer a `pasteText`
request:

```js
editor.addEventListener("continuity-request", async ({ detail }) => {
  if (detail.kind === "pasteText") editor.insertText(await hostClipboardRead());
});
```

Taking the platform's selection away also removes its drag handles, so the
editor supplies those too: two adjust handles appear once the gesture settles,
and dragging one moves that edge through the same projected-glyph mapping the
long-press drag uses. The opposite edge is frozen at the moment the handle is
grabbed rather than re-read from the live selection, which is what lets a drag
cross the anchor and keep going instead of collapsing under the finger. Handles
are coarse-pointer chrome; a mouse keeps the drag it already has. Style them
with `part="selection-handle selection-handle-start"` /
`part="selection-handle selection-handle-end"`.

## Indent guides

Set `indent-guides="on"` (property `indentGuides`, controller/React key
`indentGuides`) to paint a vertical rule at each enclosing indent level. Off by
default, so an existing host's appearance does not change under it. Colours come
from `--continuity-indent-guide` and `--continuity-indent-guide-active`.

The column semantics match the native desktop editor: a guide marks where an
*enclosing* parent's content starts, so a line at depth 1 draws none — the
body's own left edge is not a parent. A blank line inherits the columns its two
non-blank neighbours share, and the caret's line draws its deepest column in the
active colour.

## Decorating ranges

`setDecorations(id, ranges)` paints a named set of source ranges. It touches
neither selection, history, nor revision, so it is safe to call on every
keystroke:

```js
editor.setDecorations("find-match", matches.map(({ line, from, to }) => ({
  start: { line, byteInLine: from },
  end: { line, byteInLine: to },
})));
editor.setDecorations("find-active", [current]);
editor.clearDecorations("find-match");   // or clearDecorations() for all sets
```

This exists because a host cannot find its own matches in the DOM: the
projection realizes only the measured viewport plus overscan, so a browser
find, a host-side DOM walk, and CSS Highlights all see a few dozen lines of a
document that may hold thousands. Locate matches in `snapshot().text`, decorate
them all, and jump between them with `setSelections({ reveal: true })`.

Each set is themed independently. A rectangle resolves
`var(--continuity-decoration-<id>, var(--continuity-decoration))`, so defining
`--continuity-decoration-find-active` recolours that set alone, and each carries
`part="decoration decoration-<id>"`. The id must therefore be a CSS identifier —
a letter followed by letters, digits, hyphens, or underscores — and anything
else is rejected. Ranges are positions, not anchors: they do not follow edits,
so re-set them on `continuity-change`, which a find bar does anyway. Only
ranges inside the realized window paint, which is also the only part of them a
reader can see.

## Reading the visible source lines

`visibleLineRange()` returns the inclusive source-line window currently on
screen, and `continuity-viewport` publishes the same window whenever it moves:

```js
editor.addEventListener("continuity-viewport", ({ detail }) => {
  // detail: { version: 1, firstLine, lastLine }
  paintHeadingTrail(headingChainAbove(detail.firstLine));
});

// The same window, without waiting for a frame — on mount, or straight after a
// programmatic jump.
const visible = editor.visibleLineRange();   // { startLine, endLine } | null
```

This is the primitive behind scroll-linked host chrome: a sticky heading trail,
a reading-position marker, a "you are here" tick in an outline. It is the same
split as decorations — the host locates, the editor answers where the reader
is — and for the same reason. Pixels do not convert to lines outside the
projection: a heading renders at up to 1.45em, wrapped rows carry a measured
pixel hanging indent, unrealized lines are laid out against the projection's own
font metrics, and either the textarea or the touch shield owns scrolling
depending on a live `pointer: coarse` media query. There is no line height to
divide `getScrollState()` by.

What the window means:

- **Source lines**, in the same space as `Position.line`, so it composes with
  `revealRange`, `setDecorations`, and `presentationRange`.
- **Visible, not realized.** The projection realizes two further viewports in
  each direction; that larger window is deliberately not what is reported.
- **Partial lines count at both edges**, so a heading straddling the top edge is
  still the section the reader is in.
- **A wrapped line reports its own source line**, never a visual row.

The event fires for user scroll, programmatic scroll, resize, zoom or font
change, and content reflow under a stationary scroll offset. It is coalesced to
at most one per animation frame and suppressed entirely when the window has not
changed, so scrolling within one line is silent. One is emitted after
`continuity-ready` to seed a host that subscribes in its ready handler.

`visibleLineRange()` returns `null` when there is nothing to measure: before the
first layout, or while the host keeps the editor in a `display: none` tab. Zeros
would be indistinguishable from a real window at the top of a document.

## Undo history across a remount

A host that unmounts the editor per tab or pane loses the undo tree with the
engine. `exportHistory()` returns it as plain JSON and `importHistory()` adopts
it, so history can be stashed beside the scroll state:

```js
const state = {
  history: editor.exportHistory({ maxGroups: 200 }),
  scroll: editor.getScrollState(),
  selections: editor.snapshot().selections,
};
// ... after remounting with the same text ...
await editor.ready;
editor.importHistory(state.history);
editor.setSelections(state.selections, { reveal: false });
editor.restoreScrollState(state.scroll);
```

The blob carries the checksum of the content it was taken from, and importing
into different text throws rather than applying: undo replays recorded inverse
edits against the live rope, so a mismatch would rewrite the wrong bytes instead
of failing. `maxGroups` bounds the blob to the newest undo groups; omit it to
keep everything.

The shield's spacer is sized to the projection, which makes the scrollable
extent equal by construction to the height of what the reader can actually see.
That removes the whole class of bug where a document's tail was unreachable
because the textarea's own content was shorter than the projection. Scrolling
layers are inset above the command rail so their final rows cannot hide behind
it, and the caret is revealed against projected geometry rather than the hidden
textarea layout.

Mouse-style drag selection and pointer capture apply to mouse and pen input
only, and a canceled touch drops all gesture state. The scroll viewport also
survives a host `display: none` hide/show cycle (tab-style hosts that keep
the editor mounted need no scroll bookkeeping); hosts that unmount per tab
use `getScrollState()` / `restoreScrollState()`.

The textarea owns the scrollable extent, but the projection is what the reader
sees, and the same divergence that breaks platform hit-testing also makes the
projection taller than the textarea's own content: a long heading wraps into
more projected rows than raw source rows. Left alone the surplus is
unreachable — the frame clips with `overflow: hidden` and the projection is
only translated by `-scrollTop`, so the document's tail stays below the fold no
matter how far the user scrolls. The extent is reconciled after every render:
bottom padding lengthens the textarea's scrollable content while keeping the
projection tracking the text 1:1, capped so it cannot inflate the textarea's
own border box, and any surplus beyond that cap rides on the projection
transform as an offset that ramps from zero at the top to the full residual at
the scroll floor.

## Headless engine use

```js
import { readFile } from "node:fs/promises";
import { Editor, initialize } from "@continuity-editor/editor";

const wasm = await readFile(
  new URL(import.meta.resolve("@continuity-editor/editor/wasm")),
);
await initialize({ wasm });

const editor = new Editor("# Hello", savedRevision);
editor.insertText("!");
editor.executeCommand("markdown.toggle_bold");
console.log(editor.snapshot());
editor.destroy();
```

One JavaScript agent owns each `Editor` or component. Methods are synchronous
after `initialize`; no worker or async runtime is created internally.
`Editor.projection()` returns complete source/display byte maps for headless
hosts. `Editor.presentation()` is the compact DOM-oriented equivalent without
per-byte map arrays; `presentationRange(startLine, endLine)` is its
viewport-scoped form. Mutation reports include ordered `edits` splices; the
component uses them to keep its source mirror and textarea incremental.

## Support and validation

The first supported presentation lane is Chromium and Electron. The automated
lane currently exercises Chrome 150 and Electron 43.1.1; Firefox and WebKit
are not claimed. `browserslist` records a Chrome 126 / Electron 31 API floor,
not a promise that every intervening release receives its own CI job.

Mobile Chromium browsers with predictive/autocorrect keyboards (Android GBoard,
iOS Safari) are a tested lane for the IME composition path: the packed contract
suite reproduces a host document replacement landing mid-composition and asserts
the projection stays visible while a word composes (the composing line previews
the in-progress word and no other line changes), that no `continuity-change` is
emitted mid-composition and exactly one is emitted at commit, that
the textarea is never rewritten while composing, that a stale controlled echo is
dropped on `compositionend`, and that successive composed words with intervening
spaces do not compound the document.

Initialization failures reject with `ContinuityInitError`, whose message names
the required WASM MIME type and CSP token while retaining the original cause.

The package is assembled and installed from its tarball by
`cargo xtask wasm-check`. `cargo xtask browser-check` additionally runs the
packed component contract, accessibility-tree and browser performance gates,
10,000-line load/edit/newline/wrap and caret-follow coverage, then the packed Electron IPC persistence
smoke. Set `CONTINUITY_BROWSER_CPU_PROFILE=1` for a diagnostic Chrome hot-frame
report, then rerun without profiling for release measurements. The package is
not yet published to a registry. `wasm-check` also gates edited 10,000-line
viewport projection, installed package size, total JavaScript, and the lazy
entry. SDK release staging requires the full packed browser gate.

The coarse-pointer pass asserts focus-event ordering for touch tap versus
pan/cancel, but automation cannot observe a physical soft keyboard, keyboard
flash, native inertial scrolling, or mobile-browser user-activation quirks.
Run the physical Android Chrome and iOS Safari matrix in
`tests/playground/README.md` before claiming either device lane.
