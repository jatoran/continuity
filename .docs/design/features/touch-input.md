# Touch Input

On a phone or tablet the editor takes the finger off the semantic textarea
entirely. The platform's own long-press selection hit-tests that textarea, whose
layout cannot be made to agree with the projection the reader actually sees, and
on an editable element nothing can refuse the gesture. So a transparent shield
covers the editing surface, receives the touch, and owns scrolling, while the
textarea remains the semantic focus target and owns the soft keyboard, IME, and
document value beneath it. The shield leaves that focus untouched until a tap
resolves.
Everything the platform stops supplying as a result — long-press word selection,
selection drag, the clipboard bubble, drag handles, edge auto-scroll — the editor
supplies against projected geometry instead. A mouse still addresses the textarea
directly, so desktop behaviour is untouched.

## What it is

- The coarse-pointer input surface of the Web Component. A finger never touches
  the semantic textarea: a transparent shield covers the editing area, receives
  the touch, and owns scrolling.
- Touch focus is gesture-arbitrated by the component. `pointerdown` records the
  projected position without focusing; only a resolved tap commits that caret
  and synchronously focuses the textarea. A pan or cancellation never focuses.
- Exists because platform touch selection hit-tests the textarea, whose layout
  cannot be made to match the projection the reader sees.
- Fine pointers are unaffected. A mouse addresses the textarea directly and all
  desktop pointer behaviour is unchanged.

## Key concepts

- **Touch shield** (`.touch-shield`): non-editable div over the editing area.
  Pointer target and scroller on coarse pointers; inert otherwise.
- **Shield spacer**: sized to projection height; defines the scrollable extent.
- **Scroller**: whichever element owns scroll offset — shield on touch, textarea
  on mouse. One predicate resolves it for both CSS and JS.
- **Claim**: the moment a long-press becomes projection-owned.
- **Anchor range**: the whole word a long-press selected, not one endpoint.
- **Selection action bar** (`.selection-actions`): Copy / Cut / Paste /
  Select all, replacing the platform bubble the shield displaces.

## Why the textarea cannot hold the finger

The projection folds Markdown markers, renders headings at up to `1.45em`, sits
in a box 14px narrower, and applies per-line wrap indent. A `<textarea>` has no
per-line typography, so the two layouts cannot be reconciled. Platform selection
therefore picks the character under the finger in *textarea* coordinates while
the highlight is painted in *projection* coordinates — divergence grows with
every folded marker above the touch point (observed: 51 characters).

Refusing that gesture on the textarea is not possible:

| Lever | Outcome on an editable element |
|---|---|
| `selectstart` preventDefault | never raised for it |
| `contextmenu` preventDefault | arrives *after* the selection already happened |
| `user-select: none` | does not apply |

All three work on a plain div, which is why the shield exists.

## Invariants

- The projection is the only geometry a touch resolves against. Platform answers
  are a fallback, never a preference.
- Scrollable extent equals projection height by construction; the textarea's own
  content height is not a scroll authority on touch.
- Host range reveal measures the rendered projection after target realization and applies the result to the shield, not to the hidden textarea.
- `.frame.touch-scrolling` and the JS scroller predicate derive from the same
  `(pointer: coarse)` query; they cannot disagree.
- Every scrolling layer is inset above the command rail, so no row hides behind
  it.
- Post-render edit synchronization reveals the projected caret through the shield scroll owner, so touch-keyboard input cannot advance outside the visible viewport.
- Touch `pointerdown` never focuses, captures, or prevents default. A resolved
  tap commits its projected caret before synchronously focusing the textarea;
  travel, cancellation, and long-press ownership suppress the tap path.
- The engine remains the single writer of document state; clipboard fallbacks
  route their text back through an engine operation.

## Gesture lifecycle

### Tap versus pan

1. Touch `pointerdown` records the origin, pointer type, and projected source
   position and arms long-press detection. It does not focus the textarea,
   capture the pointer, prevent default, or mutate selection.
2. Touch travel is observed only to disqualify a tap. It never becomes a
   mouse-style selection drag and never prevents the shield's native pan.
3. Native scrolling normally produces `pointercancel`; cancellation clears
   projected-tap, multi-caret-collapse, composition-defer, and long-press state.
4. A synthesized `click` with a live origin inside finger slop resolves the
   tap. In that same trusted event turn the component commits the projected
   caret, then focuses the textarea so mobile user activation can open the soft
   keyboard. No timer, host callback, shadow-root inspection, or host-side
   caret hit-testing participates.
5. A click after travel, cancellation, or long-press ownership is ignored, so
   it cannot move the caret, toggle projected Markdown, or briefly open the
   keyboard.

Mouse and pen still prevent default, focus on `pointerdown`, and use pointer
capture for projection-owned drag selection.

### Long-press selection

1. `pointerdown` arms a long-press timer (320 ms, shorter than the platform's).
2. Movement beyond slop before it matures disarms it — that press was a pan.
3. On maturity: commit any open composition, hit-test the projection, select the
   word, anchor on the **whole word**, capture the pointer.
4. `pointermove` extends through projected glyphs; `touchmove` default is
   refused so the shield cannot pan out from under the drag.
5. Held near a vertical edge, the surface auto-scrolls and the selection keeps
   extending.
6. `pointerup` releases and offers the action bar.

Constraints that shape this:

- **Timer, not `contextmenu`.** Android Chrome raises no `contextmenu` for a
  long-press on a textarea; a `contextmenu`-only claim never fires on a phone.
- **Whole-word anchor.** A finger lands mid-word. Anchoring at one end and
  letting the head follow the finger collapses the selection to the fragment
  between them — the highlight appears beside the finger, and cannot grow until
  the finger passes the far edge.
- **`touchmove` refusal must be non-passive.** `touch-action` is evaluated at
  touchstart and cannot be changed mid-gesture; the press matures while the
  finger is stationary, so no scroll has begun and the refusal is honoured.
- **Auto-scroll follows from the refusal.** Refusing the scroll means the finger
  cannot otherwise reach past the viewport.
- **Source reveal freezes during a drag.** Revealing raw markers on the line the
  head enters, and re-folding the one it leaves, re-lays-out text under the
  moving finger.

## Composition

Android IMEs hold a composition open continuously across ordinary typing, so any
guard treating composition as brief is permanently on. Long-press commits the
composing run rather than refusing to claim; selection adoption proceeds
whenever engine and textarea hold the same text; a tap taken mid-composition
commits and applies the projected position before the resolved-tap focus instead
of deferring to the platform's raw-layout caret.

Enter follows the same commit-then-act rule. A line-break `beforeinput` raised
while composing commits the run and then routes to
`editor.insert_newline_smart`, so list markers, ordered numbering, task items,
empty-item outdent, and nested indentation behave identically with a composition
open or closed. Returning early instead let the textarea insert a raw newline,
which dropped the marker on every split and left the tail reading as a lazy
continuation of the item above.

`beforeinput` is the discriminating point, not `keydown`. A soft-keyboard Enter
inside a composition reports `Unidentified` / keyCode 229, so `keydown` cannot
name it; an IME candidate-commit Enter does report `key: "Enter"` but raises no
line-break `beforeinput` at all, so routing here can never turn that key into a
newline. `keydown` keeps ownership of the non-composing case, including
`Shift+Enter`, which stays a raw newline.

## Selection action bar

- Offered when a selection settles and the finger lifts, and after any
  long-press — a bare caret still reaches Select all and Paste, with Copy and
  Cut hidden rather than inert. A plain tap never summons it.
- Anchored to the painted highlight (or caret), so it can only be placed after
  the overlay paints.
- Placed above the anchor when there is room and below it otherwise, then
  clamped into the visible frame while any part of the selection is on screen.
  A selection can be taller than the viewport; anchoring on its start alone
  carried the bar off screen the moment that start scrolled away, and this bar
  is the only route to the clipboard on a phone. The clamp reads the *painted
  span* of the whole primary selection, not the anchor rectangle, which is what
  keeps it engaged once the start is above the frame; it releases when the
  selection is entirely off screen in either direction, past its end as well as
  its start. Every unclamped position that is already visible lies inside the
  clamp range, so a selection that fits on screen keeps the placement it had.
- A press on the bar must not dismiss it: pointer listeners live on the frame,
  the bar lives inside the frame, and hiding it on its own press sets
  `display: none` before the click can land.
- Selected text is captured on `pointerdown`; the tap can move focus and
  collapse the textarea selection before the action reads it.
- `preventDefault()` on `pointerdown` is mouse-only — on touch it suppresses the
  synthesized `click`.

## Selection adjust handles

Taking the platform's selection also takes its drag handles, and a selection
that cannot be nudged after the long-press that drew it has to be redrawn from
scratch. Two handles replace them:

- Offered by the same settle as the action bar, and hidden while a long-press
  drag owns the finger.
- Placed from the *painted* selection rectangles, so a selection clipped by the
  realized window puts its handle at the clipped edge — reachable, and still
  correct to drag, because the anchor comes from the engine rather than from
  geometry.
- The opposite edge is frozen at grab time, not re-read from the live selection.
  Re-deriving "the other end" flips the anchor the moment a drag crosses it, and
  the selection collapses under the finger.
- The drag aims at the glyph the handle marks, not at the finger covering it:
  the offset between the grab point and the edge is measured on `pointerdown`
  and applied for the rest of the drag.
- `touch-action: none` on the handle is what keeps the shield from panning
  instead; the handle takes pointer capture so the drag survives leaving its box.
- Coarse pointers only. A fine pointer keeps the drag it already has.

## Platform-adoption guard scope

After a projection-owned claim, adopted *platform* selections are refused for a
bounded window (`PLATFORM_GUARD_MS`). The guard sits in
`applyNativeSelection`, which is the path that pulls the textarea's own answer
into the engine — so it refuses only that. A deliberate host `setSelections` or
`revealRange` writes the engine first and pushes the result into the textarea,
and is unaffected: the guard then re-asserts the same, host-set selection.

## Clipboard fallback chain

Paste, in order:

1. `navigator.clipboard.readText()`
2. `document.execCommand("paste")` — Chrome refuses; Electron and several
   WebViews honour it
3. text this editor last copied or cut
4. `continuity-request: pasteText`

Step 3 is what keeps paste alive under a permissions policy that denies
`clipboard-read`. An embedding frame that did not delegate the capability
denies the read outright, and **a document cannot grant itself what its parent
withheld** — no page-side code lifts it. In-editor copy/paste keeps working;
only cross-application paste needs the frame to add `allow="clipboard-read"`.

Copy: Clipboard API, then the textarea's own selection via `execCommand`
(avoids the focus theft that makes a detached-textarea copy fail on a phone),
then `continuity-request: copyText`.

## API surface

- `insertText(text)` — insert at the current selection. How a host answers
  `pasteText`.
- `continuity-request: pasteText` — browser refused clipboard-read.
- `continuity-request: copyText` — `{ text }`; browser refused the write.
- `continuity-request: contextMenu` — carries `isLongPressSelection`.

## Trade-offs

- Platform magnifier and long-press paste bubble are lost on touch.
  Unavoidable: they are the platform's own UI over a surface whose geometry
  disagrees with the projection. The selection handles are replaced rather than
  lost (see above); the magnifier is not.
- Cross-application paste depends on the embedder delegating `clipboard-read`.
- Coarse-pointer behaviour is a second code path; the shield is inert on fine
  pointers so desktop risk stays at zero.

## Failure modes

- Platform steals the gesture mid-drag ⇒ `pointercancel` ⇒ pointer capture on
  claim, and adopted platform selections are refused for a bounded window after
  a claim so a correct selection is not replaced by one tens of characters away.
- Browser emits a click after pan/cancel/long-press ⇒ tap state is absent or
  travel-marked ⇒ click is ignored; selection and focus remain unchanged.
- Projection hit-test resolves nothing (tap past the last glyph, unrealized
  line) ⇒ fall back to the textarea's caret rather than leaving the engine
  stale.
- Clipboard denied ⇒ chain above ⇒ host request.

## Physical device verification

`cargo xtask browser-check` runs synthetic focus-event assertions under
coarse-pointer/touch emulation, including tap, pan with a defensive trailing
click, cancellation, long-press, selection handles, and projected caret
placement. Headless Chromium cannot verify that an OS keyboard visibly opens or
never flashes, that native inertial scrolling remains uninterrupted, or that a
specific mobile browser preserves user activation through its synthesized
click.

Before claiming this behavior on a mobile browser, run
`packages/editor/tests/playground/` on physical devices and record:

1. Android Chrome: an unfocused vertical swipe scrolls with normal inertia and
   never shows or flashes Gboard; a tap on projected heading, folded inline
   Markdown, and a wrapped list item places the visible caret and opens Gboard
   on the first tap.
2. iPhone/iPad Safari: the same swipe/tap matrix with the iOS keyboard, including
   a tap after dismissing the keyboard and a tap during an active composition.
3. Both: long-press selection and handles, links, task checkboxes, command rail,
   selection actions, and continued composition still behave as documented.

Physical Android/iOS execution is not available to the automated workspace;
device results remain a release acceptance record rather than a CI assertion.

## Key files

- shield/spacer DOM: `packages/editor/src/component_dom.js`
- scroller resolution, spacer extent, projected caret reveal:
  `packages/editor/src/scroll_surface.js`
- scroll handling and the published visible source-line window:
  `packages/editor/src/component_viewport.js`. The window is measured from
  client rectangles rather than scroll arithmetic precisely because the
  scroller switches between the textarea and the shield on a live media query
- long-press gesture, anchor range, claim state:
  `packages/editor/src/touch_selection.js`
- gesture routing, surface predicate, action execution:
  `packages/editor/src/component_pointer.js`
- tap/pan arbitration and mouse/pen capture:
  `packages/editor/src/pointer_gesture.js`
- edge auto-scroll: `packages/editor/src/drag_autoscroll.js`
- action bar and its placement clamp: `packages/editor/src/selection_actions.js`
- composing line-break routing: `packages/editor/src/native_input.js`
- adjust handles: `packages/editor/src/selection_handles.js`
- overlay chrome assembly: `packages/editor/src/component_overlays.js`
- clipboard chain: `packages/editor/src/clipboard_bridge.js`
- platform-selection adoption guard: `packages/editor/src/native_selection.js`
- shield/bar/rail CSS: `packages/editor/src/styles.js`
- textarea-path scroll extent: `packages/editor/src/scroll_extent.js`
- coarse-pointer contract: `packages/editor/tests/browser-touch-shield.mjs`
- tap versus pan/cancel focus contract:
  `packages/editor/tests/browser-touch-pointer.mjs`
- handle contract (coarse pass): `packages/editor/tests/browser-projection-chrome.mjs`
- coarse-pointer CI pass: `packages/editor/tests/browser-shield-audit.mjs`
- projection-vs-textarea divergence cases:
  `packages/editor/tests/browser-touch-selection.mjs`
- list continuation with and without an open composition:
  `packages/editor/tests/browser-list-newline.mjs`
- action bar placement math:
  `packages/editor/tests/selection-actions-placement.test.mjs`
- device harness: `packages/editor/tests/playground/`

## Relates to

- [Web Component](web-component.md) — the surface this input path belongs to.
- [Caret](caret.md) — caret placement and reveal semantics.
- [Clipboard](clipboard.md) — native Windows clipboard ownership.
- [Selection edits](selection-edits.md) — the shared engine operations selection
  gestures dispatch to.
