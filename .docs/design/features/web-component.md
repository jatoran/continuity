# Web Component

`<continuity-editor>` is the framework-neutral Chromium/Electron presentation
surface over the shared synchronous Rust engine. It preserves canonical source
text and shared display-map semantics while assigning browser-native input,
layout measurement, accessibility, and scheduling to the component's owning
JavaScript agent.

## Ownership

- One `ContinuityEditorElement` owns one WASM `Editor` on the JavaScript agent
  that constructed it. No worker is created and all engine mutation is
  synchronous after module initialization.
- The shadow-root `textarea` owns focus, composition/IME, the primary semantic
  selection, clipboard events, pointer selection, scroll position, keyboard
  navigation, and the platform accessibility node. The shared engine owns the
  complete normalized selection set; a browser overlay paints primary and
  secondary carets from measured projection glyph geometry without creating
  another text model. During IME composition the projection stays visible and
  the composing source line previews the live textarea text as a DOM-only
  update (with the composed run marked by an underline); the overlay paints the
  live textarea caret and same-line selection against those previewed glyphs.
  The reveal is line-scoped because mobile keyboards compose continuously — a
  frame-wide reveal would flash the entire document between raw source and
  rendered Markdown on every word. A composed run that cannot be mapped onto
  the projected line structure (for example one containing a newline) falls
  back to a frame-wide native text/caret/selection reveal so the writer is
  never typing blind. While a composition is active the
  textarea is never programmatically rewritten: host `value`/`replaceValue`
  writes and controlled reconciliation defer until `compositionend` and then
  apply through the revision guard (a stale echo drops). Engine renders are
  also deferred until the commit so stale engine text cannot overwrite the
  preview. The `composing`
  property exposes the active-composition state. This prevents predictive mobile
  keyboards from re-injecting and compounding the document on the space key.
- The DOM projection is visual only (`aria-hidden="true"`) and consumes the
  engine's decorations and display lines. It never becomes a second text
  model.
- The host owns persistence, durability acknowledgement, files, windows,
  context menus, link routing, dropped files, application lifecycle, and any
  Electron IPC.
- `@continuity-editor/editor/react` is a thin controlled adapter. React owns the
  rendered resource snapshot; the adapter forwards component events and uses
  revision-checked host replacement without creating another text model or
  persistence queue.
- `/controller`, `/svelte`, and `/vue` share the same controlled snapshot
  contract; `/commit-queue` is an optional host-owned scheduler, not editor
  persistence.
- Component-local render state, observers, event listeners, and abort signals
  are destroyed by `destroy()` or disconnection. Calls after destruction fail.

The native Windows app is a separate host adapter. It continues to own its
core thread, SQLite/WAL persistence, HWNDs, DirectWrite/Direct2D renderer, and
Windows input/accessibility services.

### React reconciliation

`ContinuityEditor` requires `value` plus the matching Continuity engine
`revision`. The pair is one controlled editor snapshot, not a host storage
revision. Local `continuity-change` delivery updates the React snapshot before
unrelated renders can replay old props. An unchanged pair is ignored. A changed
pair calls `replaceValue(value, revision)` after readiness; a stale revision is
reported through `onRevisionConflict` and never overwrites newer typing.

Database ETags, file mtimes, and daemon optimistic revisions stay outside the
adapter. A host may use them when persisting `detail.snapshot.text`, but must
not substitute them for the engine revision. Resource switches remount the
adapter with a stable resource key and revision zero unless the host persisted
the prior Continuity revision.

## Presentation

The selected presentation is a hybrid DOM projection with a semantic textarea
overlay. On inactive lines, Rust display-map output hides/replaces Markdown
markers and DOM spans apply block/inline decoration. The browser selection's
endpoint lines reveal canonical source immediately; intervening lines remain
projected, and a coalesced full render hides markers on inactive endpoints.
Soft wrapping is owned by the textarea, with matching typography and projection
transforms. Scroll ownership depends on the primary pointer: the textarea on a
mouse, the touch shield on a coarse pointer — see [Touch input](touch-input.md).
An overlay paints collapsed selection heads and
non-collapsed range rectangles against the currently visible projection DOM;
the textarea's native caret plus selected background and foreground remain
transparent, so raw source cannot drift onto or overpaint projected glyphs when
projected or hanging-indented lines wrap. During IME composition the same
overlay paints from the live textarea selection against the composing line's
preview instead of from the (stale mid-composition) engine selection. Overlay work is bounded
to the measured viewport plus overscan even for a whole-document selection.
After every editor-owned mutation, reconciliation preserves the current
viewport, applies the engine selection, and reveals the active selection head
with a one-line margin if it crossed the top or bottom edge. The projection
transform is synchronized in the same turn. Host-driven value replacement
preserves the host-selected viewport instead of forcing a caret jump: the
engine reconciles a full-document replacement as the minimal differing splice
(shared prefix and suffix kept), so selections, projection lines, and scroll
state outside the changed range are untouched. After a composition commit or a
textarea-first mutation (autocorrect-style input events that bypass
`beforeinput` routing), the live textarea selection is authoritative and is
adopted into the engine; the engine selection is never reasserted over it.

**Composition-to-pointer ownership rule.** While a composition is active the
engine still holds the pre-composition text, so a pointer tap cannot be applied
to it directly. Two coordinated invariants keep the tapped caret correct: (1)
pointer hit-testing maps against the *live textarea line* whenever the tapped
line is source-visible because of the composition preview — never through the
stale `sourceLines` mirror; and (2) a projected tap that completes while a
composition is still active is retained, not applied, and replayed against the
fresh snapshot only after `compositionend` reconciles the textarea mutation
exactly once (then the textarea selection is synchronized and the caret
repainted). If `compositionend` lands between `pointerdown` and the click, the
ordinary click path applies the already-live-mapped position. When the
composition preview has fallen back to the whole-frame native reveal, the native
textarea selection is preferred over projected coordinates. Deferred pointer
state is dropped on `pointercancel`, a superseding gesture, and destruction. The
rule is deliberately input-agnostic — there is no Android/Gboard/user-agent
branch. Without it, an Android predictive keyboard that holds the current word in
an open composition placed the caret one character off on a tap until the next
space committed the word.

**Hanging indent and the tab-stop grid.** A wrapped indented or list line hangs
its continuation rows beneath the first row's content, at a pixel width measured
from the rendered prefix. Expressing that as `padding-inline-start` plus a
negative `text-indent` is subtly wrong for a tab-indented line: CSS anchors tab
stops at the block's *content* edge, so the padding shifts the whole tab grid
right by the indent while the negative indent pulls the first row left by it. A
leading tab then advances to `indent mod tab-width` rather than to the first
stop, and the line's own content renders left of the rows hanging beneath it —
by an amount that changes with the font's space advance, and never for a
space-indented line. `text-indent: <width> hanging` indents every row except the
first without any inline padding, so the grid origin stays under the first row
and the two agree; the padding form remains behind `@supports` for engines
without the keyword. Lines outside the realized window are measured against the
projection's own font metrics rather than `ch` units, so an unrealized line
hangs where it will hang once realized.

**Thematic breaks.** `---` projects to an empty display line — its dashes are a
hidden marker — so the rule is drawn as a pseudo-element on the line and, like
heading sizing, is suppressed while the line carries the caret and shows raw
source.

Browser canvas measurement derives average character width and line height
after initialization and every observed resize/zoom. Before committing a new
soft-wrap width, an end-caret fast path preserves the cached distance from the
document bottom; other selections use textarea-style old/new-width mirrors so
the active caret row keeps its screen y. These values parameterize the shared
display projection; the browser remains the final layout authority.
Themes use `theme="light|dark"` and `--continuity-*` custom properties.
`prefers-reduced-motion: reduce` forces instant scrolling and zero transition
duration.

Inactive ATX/Setext headings use level-specific fixed-row font scales; active
source lines return to body scale so revealed markers do not reflow. Visible
per-line hanging indents use the shared display-map prefix boundary and browser
font measurement, so wrapped whitespace and list continuations start at the
first row's content pixel. An overwide unbroken first token stays beside its
marker and breaks only as needed; it cannot strand the marker on a row by
itself. Hovering inline or fenced code reveals an accessible Copy
control in a separate overlay. On touch-primary surfaces (`pointer: coarse`)
there is no hover, so the fenced-block Copy control is permanently visible and
survives projection rebuilds; inline controls remain tap-revealed and are not
auto-hidden. The copy path tries the async Clipboard API, falls back to a
selection-based `execCommand` copy (covers insecure LAN contexts), and only
then emits the sequenced `copyText` host request. Task markers are
pointer-toggleable.

A bottom quick-action command rail (Obsidian-style) shows on touch-primary
devices by default and is controlled by `command-rail="auto|on|off"`. It hosts
a catalog of editor-owned quick actions (undo, redo, caret up/down, line move
up/down, indent, outdent, bullet, task, numbered list, bold, italic,
strikethrough, inline code, heading cycle, blockquote, link) routed through the
same shared engine command path as keyboard shortcuts, plus any actions the
embedding host registers. `move-line-up` / `move-line-down` map to
`editor.move_line_up_block` / `editor.move_line_down_block`, so every source
line the selection covers moves as one block in one undo group. `caret-up` /
`caret-down` are the two actions with no storage-neutral command: caret motion
one *visible* row is measured against wrapped browser layout, so it is resolved
in the browser layer through the same mirror that backs multi-cursor column
addition, and a caret inside a soft-wrapped line walks that line's rows instead
of jumping a paragraph. The checkmark action (stable rail id `checkbox`)
creates a full Markdown task (`markdown.toggle_task` → `- [ ] `), not a bare
inline `[ ] ` prefix; the distinct `markdown.toggle_checkbox` command still
backs clicks on an already-rendered task marker. The bullet and task actions
share the content-relative line-start planner, so toggling a marker keeps the
caret over the same character instead of shifting it under the inserted prefix. Rail buttons cancel `pointerdown` so the semantic
textarea keeps focus and the virtual keyboard never dismisses. A gear button
opens a settings panel that enables/disables and reorders actions; the
arrangement persists in `localStorage` under `continuity-editor.command-rail`
(storage rejection degrades to a session-local arrangement), scoped by
`rail-storage-key="<name>"` so one origin can run several distinct rails. The
rail reserves its own height from the textarea viewport, hides while read-only,
and exposes `part="command-rail"` / `part="command-rail-settings"` /
`part="command-rail-button command-rail-button-<id>"` for host styling.

Hosts extend the rail through `registerRailAction(action)` (returns a disposer)
or the `railActions` property, which replaces the whole host set. Host ids are
namespaced `vendor:action`: the id is the persistence key, and reserving the
bare namespace keeps a future built-in from colliding with an arrangement a
host already saved. An action resolves in one of three ways — `command` runs an
engine command through the shared resolver, `run(element)` calls the host back,
and an action naming neither raises `continuity-request` with
`kind: "railAction"` for declarative hosts. Icons are host-built element
factories, never markup strings, so untrusted HTML never enters the shadow
root; a throwing factory or callback degrades to `continuity-error` with the
rail intact. An optional `isEnabled(snapshot)` predicate is re-evaluated on
every commit and selection change and disables the button in place rather than
removing it, so the arrangement cannot shift under a finger mid-tap.

Arrangement reconciliation is the part that makes late registration safe. A
persisted id whose action is not registered yet is retained with its slot
rather than pruned, so a host that registers after `ready` (feature flag, route
change, lazy module) returns to the position the user left it in. Pruning was
the alternative and it silently reset the user's rail on every reload.

Rail and copy-control chrome is sized in `px`, never `rem`: inside a shadow
root `rem` resolves against the embedding page's root font-size, and dense
hosts (11px roots) would shrink touch targets far below the 44–48px platform
guideline. Defaults are touch-first — 48px rail buttons in a 56px rail, 44px
settings controls — and hosts tune them through documented custom properties:
`--continuity-rail-height` (drives both the rail and the textarea bottom
inset from one value), `--continuity-rail-button-size`, and
`--continuity-rail-font-size`. Document typography continues to scale only
via `--continuity-font-size`.

A pure canvas editor is not an allowed fallback because it would require
reimplementing editing, IME, selection, and accessibility semantics.

The component keeps a revisioned JavaScript text mirror and applies exact
engine splices to the textarea instead of serializing and replacing the whole
rope per keystroke. Rust keeps an incremental tree-sitter projection cache;
the component requests a compact presentation report while `Editor.projection()`
retains complete bidirectional mappings. Line-count edits splice canonical
source placeholders and per-line projection metadata into projection DOM in
the fast frame. Edited lines become source-dirty; untouched lines retain valid
block classes, hidden markers, and inline spans during continuous typing. Those
  placeholders retain textarea-equivalent soft-wrap geometry for the whole
document; the measured pixel viewport plus two viewports of overscan is
upgraded to WYSIWYG text and inline spans. The 250 ms idle pass skips ordinary
input while its source line remains active, requests only the measured
source-line window after that line becomes inactive, and uses a complete
reconciliation for structurally significant Markdown edits. Structural
significance considers inserted and removed text alike, so deleting plain
prose stays on the incremental path. The complete pass carries the detailed
window forward and reconciles each line once against its content fingerprint
(which includes a line-relative inline-span digest), so unchanged lines keep
their DOM nodes and the pass never repaints the whole document.
`content-visibility` is forbidden on source placeholders:
unmeasured wrapped-line intrinsic heights can make the transformed projection
shorter than the textarea and leave a valid caret viewport without glyphs.

## Input normalization

| Browser input | Component action |
|---|---|
| `beforeinput` insert/delete | UTF-16 selection -> UTF-8 selection, synchronous engine operation |
| physical Enter / Shift+Enter `keydown` | shared smart-indent/list/task continuation / raw newline; default textarea insertion canceled |
| non-keyboard `beforeinput` line break | shared smart-indent/list/task continuation; a line break raised while an IME composition is open commits the composing run first, so the planner runs against text the engine matches. Composed text itself (`insertCompositionText`) is still left to the textarea. See [Touch input](touch-input.md) |
| composition start/input/end | native textarea composition (host writes and engine renders deferred) with a line-scoped live preview of the composing line, then one minimal-splice reconcile that syncs without re-applying edits so the composed run does not double, and adopts the textarea caret so it never jumps |
| Ctrl/Cmd+Z, redo chords | engine undo/redo |
| portable command chord | resolve by shortcut policy, then execute the shared typed engine operation |
| Tab / Shift+Tab | shared engine line indent / outdent; default unit is one tab, matching the native default |
| Ctrl/Cmd+Alt+Up/Down | add and activate a visual-row caret; fall back to a source row when layout measurement is unavailable |
| Ctrl/Cmd+click | activate Markdown links first; otherwise add and activate the pointer caret |
| Escape | clear secondary carets first; with one caret, arm focus traversal |
| Escape, then Tab / Shift+Tab | one-shot browser focus traversal out of editable mode |
| touch pointer (`pointerType: "touch"`) | routed through the touch shield, never the textarea: `pointerdown` records without focusing/capturing/preventing; native drag pans; timed long-press claims a projection-owned word selection; a resolved tap commits the measured projected caret and synchronously focuses the textarea in the trusted click turn. See [Touch input](touch-input.md) |
| touch selection action | Copy / Cut / Paste / Select all bar replacing the platform bubble, clamped into the visible frame while any part of the selection is on screen; `pasteText` / `copyText` host requests when the browser refuses clipboard access |
| simple pointer click / Shift+click | measured projection glyph -> display-map segment -> canonical source caret/range |
| double-click | measured canonical source position -> shared Rust word selection |
| triple-click | measured canonical source position -> shared Rust line selection, excluding the line ending |
| primary-button drag | captured pointer -> measured projection range -> semantic textarea and engine selection |
| keyboard selection/focus | native textarea state -> engine selection/render scheduling |
| Arrow/Home/End/Page navigation | browser default movement -> animation-frame selection reconcile and projected-caret repaint |
| copy/cut/paste/text drop | all engine ranges newline-joined; normalized plain text -> engine mutation |
| file drop | `continuity-request: filesDropped` |
| Ctrl/Cmd+click Markdown link | `continuity-request: openLink` |
| code-copy fallback | `continuity-request: copyText` |
| context menu | `continuity-request: contextMenu` |
| scroll/resize/animation frame | projection transform, measurement, and coalesced render |

Editable mode owns Tab by default because indentation is an editor operation,
not a host concern. Shift+Tab outdents. Escape arms one-shot Tab/Shift+Tab
browser focus traversal; read-only mode releases Tab directly. The semantic
textarea describes this gesture to assistive technology. Form-like hosts may
set `tab-behavior="focus"` to opt into direct Tab traversal.

### Shortcut policy

`shortcut-policy="browser-safe"` is the default. It preserves Chrome's
documented UI/DevTools accelerators, including Ctrl/Cmd+E, K, R, Shift+R, J,
Shift+J, U, and Shift+C; Continuity does not pretend those chords are portable
page APIs. `editor-first` claims every Continuity binding that reaches the
component and is recommended for Electron or another controlled shell. `none`
releases all default command shortcuts to the host. Tab indentation and its
Escape traversal gesture remain governed separately by `tab-behavior`.

`shortcutBindings` is a host overlay keyed by chords such as `Mod+E`. A command
ID adds or replaces a binding and `null` explicitly unbinds it. Explicit host
bindings override `browser-safe`, but a normal browser can still consume a
browser-UI accelerator before page dispatch. `executeCommand(commandId)` is
the deterministic alternative and rejects desktop-owned file/window/pane/tab
commands. Ctrl+Shift+Left/Right, ordinary arrows, selection, and clipboard
remain textarea-native; explicit structural chords such as
Ctrl+Shift+Up/Down use the Rust engine.

This boundary follows [Chrome's published shortcuts](https://support.google.com/chrome/answer/157179),
the [Chrome DevTools shortcut reference](https://developer.chrome.com/docs/devtools/shortcuts),
the [W3C cancelable-keydown contract](https://www.w3.org/TR/uievents/#event-type-keydown),
and Electron's [`before-input-event` guidance](https://www.electronjs.org/docs/latest/tutorial/keyboard-shortcuts/).

## Public API

Importing `@continuity-editor/editor` defines the custom element. Hosts can
instead call `defineContinuityEditor(registry)` explicitly; definition is
idempotent.

- `ready: Promise<this>` resolves after the WASM editor and first projection
  exist.
- `value` sets the initial value before ready; later writes use a
  revision-checked replacement against the live revision.
- `initialRevision` restores the host-owned revision before readiness; it must
  be a non-negative safe integer and cannot reset a live editor.
- `readOnly` and the `readonly` attribute update both engine input policy and
  `aria-readonly`.
- Standard `spellcheck` property/attribute state mirrors to the semantic
  textarea immediately; browser default is enabled and hosts may set `false`.
- `shortcutPolicy` / `shortcut-policy` select `browser-safe`, `editor-first`,
  or `none`; `shortcutBindings` overlays individual chords.
- `executeCommand(commandId, timestamp?)` synchronously applies a supported
  editor-owned command through the shared Rust operation resolver.
- `syntax="markdown|plain"` selects projected Markdown or identity plain text.
- `indent-guides="on|off"` (property `indentGuides`, controller/React key
  `indentGuides`) paints vertical rules at each enclosing indent level. Off by
  default so an existing host's appearance does not change under it. Column
  semantics mirror the desktop painter: a guide marks where an *enclosing*
  parent's content starts, the body-left-edge column is suppressed, a blank line
  inherits the columns its two non-blank neighbours share, and the caret's line
  draws its deepest column in the active colour.
- `setDecorations(id, ranges)` paints one named set of source ranges without
  touching selection, history, or revision; `clearDecorations(id?)` removes one
  set or all of them. The id must be a CSS identifier because it names both the
  theming property `--continuity-decoration-<id>` and the shadow part
  `decoration-<id>`. Ranges are positions, not anchors: a host re-sets them
  after a change, which a find bar does anyway. Only ranges inside the realized
  viewport window paint, which is also the only part of them a reader can see.
- `exportHistory(options?)` returns the undo history as plain JSON and
  `importHistory(history)` adopts it. Hosts that unmount per document (tabs,
  panes) stash it beside the scroll state. The blob carries the content
  checksum and an import into different text is refused rather than replaying
  recorded inverse edits over the wrong bytes. `maxGroups` bounds the blob to
  the newest undo groups.
- `command-rail="auto|on|off"` controls the bottom quick-action rail; `auto`
  (default) shows it only on touch-primary devices.
- `registerRailAction(action)` adds one host action to the rail and returns its
  disposer; `railActions` reads or replaces the whole host set (also a React
  adapter prop and a controller configuration key).
- `rail-storage-key="<name>"` scopes the persisted rail arrangement so several
  distinct rails can share one origin.
- `listBuiltInRailActions()` enumerates the built-in ids, labels, engine
  commands, and default enablement a host can arrange around.
- `setSelections`, `revealRange`, scroll capture/restore, and
  `linearMemoryBytes` support multi-document host state and telemetry.
  Host reveal waits for the selection's projection render, realizes the target region, and measures rendered caret or range rectangles against the active textarea or touch-shield scroller.
  `nearest` keeps a fitting target fully visible with one rendered-row margin; `center` centers the target subject to scroll bounds.
  `setSelections(..., { reveal: true })` shares the same primitive for the primary selection head.
  Reveal changes neither document revision nor undo history.
  Every document edit queues the same post-render projected-caret check, so the active row remains visible even when wrapping or Markdown block geometry diverges from the textarea.
  A host that keeps the editor mounted and toggles `display: none` (tab
  switching) needs no scroll bookkeeping: the component ignores the
  zero-width hide and reasserts the tracked viewport when layout returns.
  Hosts that unmount per tab capture `getScrollState()` before unmount and
  call `restoreScrollState(state)` after remount.
- `visibleLineRange()` returns the source lines currently on screen, or `null`
  before first layout. See [The visible window](#the-visible-window).
- `snapshot()` returns canonical text, revision, selections, and read-only
  state.
- `replaceValue(value, expectedRevision, timestamp?)` rejects stale host state
  with `RevisionConflictError`, and returns `null` when it defers because an IME
  composition is active.
- `composing` reports whether a browser IME composition is currently active.
- `focus(options?)` focuses the semantic textarea.
- `destroy()` aborts listeners, disconnects observers, destroys the engine,
  and emits `continuity-destroy`.

Events are composed and bubble across the shadow boundary:

- `continuity-ready` carries protocol version and initial snapshot;
- `continuity-change` carries protocol version `1`, component-local sequence,
  normalized source, stable `commitOrigin: user|host`, `Change`, and
  post-change `Snapshot`; hosts persist only `user`;
- `continuity-request` carries a sequenced host request — `openLink`,
  `contextMenu` (with `isLongPressSelection`), `filesDropped`, `copyText`, and
  `pasteText`. A host answers `pasteText` with `insertText(text)`; see
  [Touch input](touch-input.md);
- `continuity-frame` carries revision and input-to-paint-ready timing;
- `continuity-viewport` carries the source-line window the reader can see, as
  `firstLine` / `lastLine`. See [The visible window](#the-visible-window);
- `continuity-error` carries a recoverable error;
- `continuity-destroy` confirms teardown.

A change event acknowledges in-memory mutation only. Hosts must persist events
in sequence and define their own durable acknowledgement.

## The visible window

`visibleLineRange()` returns `{ startLine, endLine }` - the inclusive source
lines with any pixel on screen - or `null` when there is nothing to measure
(before first layout, or while the host keeps the editor in a `display: none`
tab). `continuity-viewport` publishes the same window as `firstLine` /
`lastLine` whenever it moves.

This exists for the same reason `setDecorations` does: the host owns the model
and the editor owns the geometry. A host painting scroll-linked chrome - a
sticky heading trail, a reading-position marker - has the heading model already
and needs exactly one fact it cannot obtain. Pixels do not convert to lines
outside the projection: a heading renders at up to 1.45em, wrapped rows carry a
measured pixel hanging indent, unrealized lines are laid out against the
projection's own font metrics, and `scroll_surface.js` switches the scroller
between the textarea and the touch shield on a live `pointer: coarse` media
query. There is no line height to divide `getScrollState()` by, and the shadow
DOM is `mode: "open"` only so hosts can inspect - not so they can re-derive
geometry that would break on the next projection change.

The contract that makes it useful:

- **Source lines**, in the same space as `Position.line`, so the window
  composes with `revealRange`, `setDecorations`, and `presentationRange`.
- **Visible, not realized.** The projection realizes two further viewports in
  each direction; publishing that window would name a line two screens from the
  reader, which is the bug the feature exists to avoid.
- **Partial lines count at both edges.** A heading straddling the top edge is
  still the section the reader is in; reporting the first fully visible line
  would make host chrome flicker as that heading scrolls out.
- **A wrapped line reports its own source line**, never a visual row, because
  the projection holds one element per source line and its continuation rows
  live inside that element.
- **Every cause of movement publishes**: user scroll, programmatic scroll,
  resize, zoom or font change, and reflow under a stationary scroll offset.
  These reach either the scroll handler or a render, and both probe.
- **At most one publication per animation frame**, and none when the window is
  unchanged. Reading a client rectangle forces layout, so probing per scroll
  event would stall the scroll and hand hosts more repaints than they can use.
- **Seeded once after `continuity-ready`**, so a host subscribing in its ready
  handler gets the opening window without polling.

The editor deliberately does not paint the trail itself. The heading model,
the jump target, and the chrome's own typography are product decisions; this is
the primitive underneath them.

`<continuity-renderer>` is the selectable static Markdown/plain surface. It
shares WASM presentation and typography without owning an editing textarea.

## Accessibility

The textarea is exposed as one named multiline textbox; the projection is
removed from the accessibility tree to prevent duplicate text. `aria-label`
propagates from the custom element, `aria-readonly` follows state, and an
`aria-describedby` instruction explains Tab indentation plus Escape-then-Tab
focus traversal. Browser tests require this description in Chromium's full
platform accessibility tree rather than relying only on DOM attributes. This
implements [WCAG 2.2 SC 2.1.2](https://www.w3.org/WAI/WCAG22/Understanding/no-keyboard-trap.html):
keyboard focus remains escapable and the nonstandard exit method is advised.

Manual acceptance for each claimed browser/OS pair verifies:

1. the editor is announced as a named multiline edit field;
2. character, word, and line navigation announce the expected text;
3. typing, selection, replacement, clipboard, undo, and redo are announced;
4. Tab/Shift+Tab indent/outdent, while Escape then Tab/Shift+Tab exits without
   trapping focus;
5. composition input is neither duplicated nor omitted;
6. continued typing, paste, undo, and redo across the top and bottom edges keep
   the active caret visible without manual scrolling;
7. read-only state and host status updates are announced;
8. teardown does not leave a focusable ghost node.

Run `cargo xtask browser-check`, then start the generated manual browser page
with `node manual-server.mjs .` in
`target/wasm-sdk/browser-consumer`. Start the generated Electron host with
`npm start` in `target/wasm-sdk/electron-consumer`. The Electron host exposes
Focus, Toggle read-only, and Destroy/Recreate controls plus a polite live
status region. After Destroy, Tab must skip the removed editor; Recreate must
restore the last snapshot and revision.

## Support and gates

The supported presentation family is Chromium/Electron. Automated CI uses the
runner's installed Chrome and pinned Electron; Firefox and WebKit require
their own IME, accessibility, rendering, and E2E evidence before any general
web-platform claim.

`cargo xtask browser-check` builds and clean-installs the npm tarball, runs 287
browser behavior assertions, audits the platform accessibility tree, performs
real CDP proportional-font wrapped-projection hit-testing, visual-caret
alignment, exact wrapped Shift+click/drag selection and multi-row highlight
painting, physical Enter smart-newline,
physical Shift+Enter raw-newline, Tab/Shift+Tab editing,
Ctrl+Shift+Left word selection, editor-first Ctrl+R/Ctrl+E interception with content-relative task carets, and
Escape-then-Tab focus traversal at 1.25 device scale, and inserts real text at
the end of an offscreen long document to require caret reveal plus same-turn
projection-scroll synchronization. The component contract additionally pins
smart indentation/list/task continuation, multi-cursor creation/edit/Escape,
modifier-click precedence, task-marker toggling, hanging-wrap metadata,
heading hierarchy, inline/fenced copy controls, large-paste viewport coverage,
large-range viewport-bounded highlights, the line-scoped IME composition
preview (whole-document projection stability while a mobile keyboard composes,
commit-granular change events, and the frame-wide native-reveal fallback for
unmappable composed runs),
the bottom command rail (engine command execution with retained textarea
focus, settings enable/disable/reorder/reset with localStorage persistence,
read-only and `command-rail` policy visibility, permanently visible touch
code-copy controls, block line movement, visible-row caret motion inside a
soft-wrapped line, host action registration/disposal/replacement across
command, callback, and request routes, id validation, enablement predicates,
per-button parts, late-registration slot retention, and `rail-storage-key`
scoping),
continuous-typing projection stability, and public spellcheck mirroring,
enforces browser-specific latency budgets, and launches the Electron IPC
persistence smoke twice to prove main-process Ctrl+E routing and
revision/sequence continuity across restart.
Canonical measurements and the manual matrix live in
`../../development/archive/embeddable_presentation_spike_2026-07-17.md`.

## Key files

- `packages/editor/src/component.js` — custom element, normalized input, API,
  events, lifecycle.
- `packages/editor/src/keyboard.js` — shortcut dispatch, editor-owned Tab
  policy, and one-shot focus escape.
- `packages/editor/src/shortcuts.js` — default bindings, conflict policy, and
  host binding normalization.
- `packages/editor/src/projection.js` — display-map consumption, viewport DOM
  realization, marker reveal, scroll synchronization, and the live-composition
  hit-test override.
- `packages/editor/src/projection_measure.js` — browser typography and textarea
  caret-position measurement for scroll reveal, resize anchoring, and viewport
  persistence.
- `packages/editor/src/component_pointer.js` — pointer-down/move/cancel/click and
  projected/pending/vertical/secondary selection application over the element's
  shared pointer context.
- `packages/editor/src/input_sync.js` — incremental textarea splices, selection,
  caret-follow synchronization.
- `packages/editor/src/composition_preview.js` — line-scoped IME composition
  preview, live composition caret/selection painting, native-reveal fallback.
- `packages/editor/src/command_rail.js` — rail attachment, rendering, action
  dispatch, enablement, and visibility policy.
- `packages/editor/src/command_rail_registry.js` — built-in catalog, host action
  validation/registration, arrangement resolution and persistence.
- `packages/editor/src/command_rail_settings.js` — rail settings panel
  (enable/disable, reorder, reset).
- `packages/editor/src/component_transfer.js` — clipboard, drop, and
  context-menu event adapters over the shared pointer context.
- `packages/editor/src/pointer_hit_test.js` — grapheme-range measurement and
  projected-display to canonical-source pointer mapping.
- `packages/editor/src/pointer_gesture.js` — captured click-count, Shift+click,
  and drag selection ownership.
- `packages/editor/src/scroll_surface.js` — which element owns scrolling and shield extent.
- `packages/editor/src/projection_reveal.js` — post-render caret and range measurement, alignment, and scroll-owner correction.
- `packages/editor/src/touch_selection.js` — timed long-press claim, whole-word
  anchor range, and platform-adoption guard state.
- `packages/editor/src/drag_autoscroll.js` — edge auto-scroll during a selection
  drag.
- `packages/editor/src/selection_actions.js` — touch selection action bar.
- `packages/editor/src/selection_handles.js` — touch selection adjust handles
  and the anchor frozen for one handle drag.
- `packages/editor/src/component_overlays.js` — assembly of the chrome anchored
  to painted selection geometry: action bar, handles, host decorations.
- `packages/editor/src/decorations.js` — host range decoration sets and their
  per-set theming property.
- `packages/editor/src/overlay_rects.js` — per-row client rectangles for one
  source range, shared by the selection overlay and decorations.
- `packages/editor/src/indent_guides.js` — indent-guide columns, blank-line
  carry-over, and the per-line background they paint into.
- `packages/editor/src/wrap_layout.js` — measured hanging indent and the font
  metrics unrealized lines are measured against.
- `packages/editor/src/clipboard_bridge.js` — clipboard read/write fallback
  chain and host delegation.
- `packages/editor/src/native_selection.js` — `selectionchange` adoption of
  platform-driven caret moves.
- `packages/editor/src/scroll_extent.js` — textarea-path scroll extent when the
  projection outgrows the textarea's own content.
- `packages/editor/src/component_listeners.js` — which DOM node owns each
  listener.
- `packages/editor/src/component_composition.js` — composition lifecycle,
  including mid-composition commit.
- `packages/editor/src/host_navigation.js` — host selection and viewport API.
- `packages/editor/src/selection_overlays.js` — viewport-bounded projected
  range and caret painting.
- `packages/editor/src/visual_carets.js` — primary/secondary caret placement
  against current projection glyph rectangles.
- `packages/editor/react.js` — React event/ref/property bridge and controlled
  reconciliation lifecycle.
- `packages/editor/src/controlled.js` — framework-independent snapshot
  initialization, configuration, and conflict-preserving replacement.
- `packages/editor/controller.js` and `src/controller.js` — public
  framework-neutral attach/synchronize/dispose lifecycle and event forwarding.
- `packages/editor/tests/browser-performance.mjs` — 1,500/10,000-line packed
  Chromium latency and projection budgets.
- `packages/editor/src/styles.js` — shadow presentation, themes, selection,
  visual carets, composition fallback, reduced motion.
- `packages/editor/tests/browser-suite.mjs` — browser contract and budgets.
- `packages/editor/tests/browser-runner.mjs` — Chromium/CDP and accessibility
  audit.
- `apps/electron-example/` — minimal embedder example.
- `apps/desktop-web/` — distributable Electron host with durable storage,
  files, menus, updates, packaging, and artifact smoke.
