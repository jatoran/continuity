# @continuity-editor/editor changelog

The embeddable SDK versions independently of the Continuity Windows desktop
application; `CHANGELOG.md` at the repository root is the desktop product's.
Release notes for `0.2.16` and earlier live in
[`EMBEDDING.md`](https://github.com/jatoran/continuity/blob/main/EMBEDDING.md),
which remains the cross-surface router.

## 0.2.27

### Fixed

- **`revealRange()` now reveals the rendered Markdown range, not the semantic textarea approximation.**
  Host navigation is deferred until the projection has rendered the requested selection, then measured from its projected caret or range rectangles.
  `align: "nearest"` keeps a fitting target fully visible with one rendered-row edge clearance, while `align: "center"` centers it as closely as scroll bounds permit.
  The same primitive now backs `setSelections(..., { reveal: true })`.
  It resolves the live scroll owner, so desktop textarea scrolling and coarse-pointer touch-shield scrolling behave consistently.
  Text, revision, and undo history are unchanged.

- **Desktop projection realization now uses compensated projection scroll coordinates.**
  When rendered Markdown is taller or shorter than the textarea, the detailed viewport and the painted transform now agree at the document tail without unreachable content or blank overscroll.
  This keeps late wrapped-line targets measurable after programmatic jumps.

- **Typing now keeps the rendered caret inside the viewport.**
  Every edit reveals the caret from its post-render Markdown geometry instead of trusting the semantic textarea row.
  The same behavior applies to the desktop textarea scroll owner and the coarse-pointer touch shield, including wrapped continuation rows.

## 0.2.21

### Fixed

- **A touch selection no longer raises the Android keyboard.** With the keyboard
  dismissed, long-pressing a word and dragging to extend the selection brought it
  back up over the text being selected.

  This is not the focus defect `0.2.18` fixed, and that fix is still in place:
  the touch path calls no `focus()` until a tap resolves. Keyboard visibility on
  Android simply is not a function of DOM focus. The system back gesture hides
  the IME *without* blurring, so the editor's textarea is still the focused
  element afterwards, and Chrome re-raises the keyboard for any touch that
  resolves against a focused editable. Focus never moves, so no focus policy can
  reach it.

  The editor now holds its internal textarea at `inputmode="none"` on a coarse
  pointer, which is the one state Chrome will not raise the IME from, and lifts
  it only where a touch has already been resolved as typing: a completed tap, or
  an explicit insert (`insertText()`, and the built-in paste action). A
  long-press claim and an adjust-handle grab put it back, so a gesture that
  begins as selection cannot end in a keyboard.

  Two notes for hosts:

  - **Desktop is untouched.** The gate is applied only for `pointer: coarse`;
    mouse and pen still focus on pointerdown, and `inputmode` means nothing to a
    physical keyboard.
  - **`inputmode="none"` is readable, and is the honest signal.** A host that
    manages the soft keyboard itself - deciding whether a focused field is what
    is holding it up - can treat the attribute as "this editor raises no
    keyboard right now" rather than inferring it from focus.

## 0.2.20

### Added

- **The visible source-line window is readable.** `visibleLineRange()` returns
  `{ startLine, endLine }` - the inclusive source lines with any pixel on
  screen - and `continuity-viewport` publishes the same window as
  `{ version: 1, firstLine, lastLine }` whenever it changes. This is the
  primitive a host needs to paint scroll-linked chrome of its own, such as a
  sticky heading trail; the editor does not paint one.

  Semantics, which matter more than the shape:

  - **Source lines**, in the same space as `Position.line`, so the window
    composes with `revealRange`, `setDecorations`, and `presentationRange`.
  - **Visible, not realized.** The projection realizes two further viewports in
    each direction so scrolling has DOM ready ahead of it. That window is not
    what is reported; it would name a line two screens from the reader.
  - **Partial lines count at both edges.** A heading straddling the top edge is
    still the section the reader is in, and reporting the first fully visible
    line would make a host's sticky chrome flicker as that heading scrolls out.
  - **A wrapped line reports its own source line**, never a visual row.
  - **Published for every cause of movement**: user scroll, programmatic scroll
    (`revealRange`, `restoreScrollState`, caret reveal), resize, zoom or font
    change, and content reflow under a stationary scroll offset.
  - **At most one event per animation frame**, and none at all when the window
    has not changed, so scrolling within one line is silent.
  - **Seeded once after `continuity-ready`**, so a host subscribing in its ready
    handler receives the opening window without polling.
  - `visibleLineRange()` returns `null` when there is nothing to measure:
    before the first layout, or while the host keeps the editor in a
    `display: none` tab. Zeros would be indistinguishable from a real window at
    the top of the document.

### Notes

- Additive only. No behavior change to existing surfaces.

## 0.2.19

### Fixed

- **Enter continues a list marker while an IME composition is open.** Android
  keyboards hold a composition open on the word under the caret across ordinary
  typing, and the `beforeinput` router returned before its line-break entries
  for anything composing - without preventing the default. The textarea inserted
  a raw newline, the list-aware planner never ran, and the split lost its
  marker, so the tail rendered as a lazy continuation of the item above. Enter
  now commits the open composition and runs `editor.insert_newline_smart`, the
  same commit-then-act order a tap already used. Marker continuation, ordered
  numbering, task items resetting to `- [ ] `, empty-item outdent, and nested
  indentation all behave identically with a composition open or closed.
  `Shift+Enter` still inserts a raw newline. `beforeinput` is the discriminating
  point rather than `keydown`, so an IME candidate-commit Enter - which reports
  `key: "Enter"` while composing but raises no line-break `beforeinput` - is
  still left to the IME.
- **The touch selection action bar stays on screen for a selection taller than
  the viewport.** Its vertical position was derived only from the selection's
  own start and was never clamped, so once that start scrolled off the top the
  Copy / Cut / Paste / Select all bar left with it and the only way back to the
  clipboard was to scroll up and find it. The bar is now clamped into the
  visible text area while any part of the selection is on screen, and follows
  the scroll from there; the clamp is released once the selection is entirely
  off screen in either direction. A selection that fits on screen keeps exactly
  the placement it had.

### Notes

- No public API changes.

## 0.2.18

### Fixed

- **Touch scrolling no longer focuses the editor or flashes the soft keyboard.**
  Touch `pointerdown` now records the gesture without focusing, capturing, or
  preventing the shield's native scroll. A resolved tap commits its measured
  projected caret and synchronously focuses the textarea in the trusted click
  turn; pan, cancellation, and long-press clicks stay unfocused. Mouse and pen
  focus/capture behavior is unchanged, and hosts need no shadow-root or
  caret-hit-test workaround.

### Notes

- No public API changes.

## 0.2.17

### Fixed

- **Thematic breaks render.** `---` projects to an empty display line — its
  dashes are a hidden marker — so it read as a blank line. It now draws a rule,
  suppressed on the caret's own line so the raw source comes back under the
  caret, the way heading sizing already behaved.
- **Tab-indented wrapped rows hang under their own content.** The hanging indent
  was expressed as `padding-inline-start` plus a negative `text-indent`. CSS
  anchors tab stops at the block's *content* edge, so the padding shifted the
  whole tab grid right by the indent while the negative indent pulled the first
  row left by it: a leading tab advanced to `indent mod tab-width` instead of to
  the first stop, and a nested bullet's own text rendered left of the rows
  hanging beneath it — by an amount that changed with the font's space advance,
  and never on a space-indented line. The indent is now
  `text-indent: <width> hanging`, which indents every row except the first
  without inline padding, so the grid origin stays under the first row. The
  padding form remains behind `@supports` for engines without the keyword.
- **Unrealized lines hang where they will hang once realized.** Lines outside
  the measured viewport window fell back to `ch` units with a hard-coded
  four-column tab. `ch` is the advance of `0`, which in a proportional font is
  neither the space advance nor the tab stop. They are now measured against the
  projection's own font metrics, read once per render pass.

### Added

- **`indent-guides="on"`** (property `indentGuides`, controller/React key
  `indentGuides`) paints vertical rules at each enclosing indent level. Off by
  default. Themed with `--continuity-indent-guide` and
  `--continuity-indent-guide-active`. Column semantics mirror the native
  desktop painter: a guide marks where an *enclosing* parent's content starts,
  the body-left-edge column is suppressed, a blank line inherits the columns its
  two non-blank neighbours share, and the caret's line draws its deepest column
  in the active colour.
- **`setDecorations(id, ranges)` / `clearDecorations(id?)`** paint named sets of
  source ranges without touching selection, history, or revision. This is what a
  host-side find bar needs: the projection realizes only the measured viewport,
  so offscreen matches are not in the DOM for a browser find, a host DOM walk, or
  CSS Highlights to reach. Each set is themed through
  `--continuity-decoration-<id>` (falling back to `--continuity-decoration`) and
  exposed as `part="decoration decoration-<id>"`. Ids must be CSS identifiers
  because they name both. Ranges are positions, not anchors: re-set them after a
  change.
- **Touch selection adjust handles.** The touch shield displaces the platform's
  own handles, so a selection was whatever the long-press produced and could not
  be nudged. Two handles now appear once the gesture settles and drag their edge
  through the same projected-glyph mapping the long-press drag uses, pivoting on
  the opposite edge frozen at grab time so a drag can cross the anchor. Coarse
  pointers only; `part="selection-handle selection-handle-start|-end"`.
- **`exportHistory(options?)` / `importHistory(history)`** move the undo tree
  across an unmount as plain JSON, for hosts that unmount the editor per tab or
  pane. The blob carries the content checksum and an import into different text
  is refused rather than replaying recorded inverse edits over the wrong bytes.
  `maxGroups` bounds it to the newest undo groups.

### Notes

- The installed-JavaScript budget moves 288 KiB -> 320 KiB for the above.
- No breaking changes. Every addition is additive in `index.d.ts`.
