# @continuity-editor/editor changelog

The embeddable SDK versions independently of the Continuity Windows desktop
application; `CHANGELOG.md` at the repository root is the desktop product's.
Release notes for `0.2.16` and earlier live in
[`EMBEDDING.md`](https://github.com/jatoran/continuity/blob/main/EMBEDDING.md),
which remains the cross-surface router.

## 0.2.36

### Fixed

- **Block toggles no longer absorb the line below them.**
  With the caret on `alpha` in

  ```text
  alpha
  beta
  ```

  pressing the bullet toggle (`editor.toggle_bullet_at_line_start`, `Ctrl+R`, or the command rail's bullet action) produced `- alpha` followed by an untouched `beta` - and `beta` silently became part of the bulleted item.
  That is CommonMark working as specified: a plain line following a marked line is *lazy continuation* text and belongs to the block above it.
  It is not what a writer means. In a notes editor a newline is a line, and marking one line must not restyle the next.

  The defect was never bullet-specific. Every block toggle is the same line-prefix rewrite over the covered lines, blind to what follows: `MarkdownToggleBullet`, `MarkdownToggleNumbered`, `MarkdownToggleTask`, and `MarkdownWrapInBlockquote` all absorbed the following line, and the reverse direction had the mirror defect - stripping the marker from the middle item of a list dropped that line into the *previous* item (`- a` / `- b` / `- c`, unbullet `b`, and `b` became continuation text of item `a`). Demoting a heading to plain text merged it with the paragraph beneath it the same way.

  Toggling a block marker is now scoped to the lines it rewrites. Where a rewrite would otherwise pull an untouched neighbour into the toggled block, or newly join it to one, the toggle inserts a blank-line separator as part of the same edit.

  ```text
  alpha        →   - alpha
  beta                          (blank separator)
                   beta
  ```

  **The resolution is a paragraph split, not a continuation indent.** Indenting the following lines under the new item (`- alpha` / `  beta`) was the other candidate; it makes the absorption explicit instead of preventing it, which is exactly the behaviour being reported as wrong. Splitting keeps the untouched line rendering as what the writer wrote.

  **Separators are inserted, never auto-removed.** A blank line the writer typed is indistinguishable from one a toggle inserted, and deleting it on the reverse toggle would silently merge paragraphs the writer had separated. So toggle-on then toggle-off is byte-identical whenever no separator was needed - a single-line paragraph, the last line of a paragraph, a line next to an existing list - and when one was needed the text is otherwise byte-identical with the separator left in place.

  Everything lands in one undo group: a single undo restores the marker, the separator, and the selection together.

  Toggles that open no block are unaffected. `MarkdownToggleCheckbox` writes `[ ] `, which is not a list marker, and setting a heading needs no separator because an ATX heading is a leaf block that never continues into the line below.

### Changed

- `continuity-engine` gains an internal `edit_block_scope` module carrying the CommonMark paragraph-interruption predicates shared by every block-marker planner. No public API change; the Rust, C ABI, Python, and npm surfaces are unchanged from `0.2.35`.

## 0.2.35

### Fixed

- **Selecting text no longer dismisses an Android keyboard that is already up.**
  `0.2.21` stopped a touch selection from *raising* the keyboard by holding the internal textarea at `inputmode="none"` during a selection gesture.
  That attribute is not symmetric: applied to a field that is focused with the IME showing, it does not refuse a future raise, it takes the keyboard away.
  So the fix for the reader broke the writer - long-pressing a word or dragging an adjust handle mid-sentence closed the keyboard being typed on.

  The gate is no longer a reflex. The editor now tracks keyboard state explicitly and closes the gate only when there is no keyboard to lose:

  - **Typing intent** - the editor resolved a gesture as "type here" and asked for the IME. A request, not an observation.
  - **Visual-viewport occlusion** - the signal Android actually gives for the IME: the visual viewport loses height while the layout viewport does not. This is the observation, and it corrects the request in both directions.

  Where there is no evidence either way, the answer is "keyboard down", which is the conservative side and preserves `0.2.21` exactly: a keyboard dismissed with the system back gesture still cannot be re-raised by a long-press, a selection drag, or an adjust-handle grab.
  A dismissal observed through the viewport now re-arms the gate immediately, closing the window in which the next touch could raise a keyboard nobody asked for.

- **The first tap inside an existing selection raises the keyboard instead of destroying the selection.**
  On a phone, a selection made with the keyboard down had no usable next step: every route to acting on it - typing over it, and every button on the selection action bar - needs the keyboard, and the only gesture that raises one also collapsed the selection on the way.

  While the keyboard is down, a resolved touch tap that lands inside the highlight now buys the keyboard and nothing else.
  The range, the painted highlight, and Copy / Cut / Paste / Select all all survive it.
  The grant is deliberately narrow: coarse pointers only, inside the highlight only, and once per selection - the next tap means what a tap normally means, so a reader can always tap their way out of a selection.
  A tap outside the selection is unchanged: it places a caret and raises the keyboard, as it always did.

  Desktop is untouched. Mouse, pen, and physical-keyboard behaviour are identical; a mouse click inside a selection still collapses it, and `inputmode` still means nothing to a physical keyboard.

- **Tearing the editor down mid-composition no longer discards the composing run.**
  A composing run is withheld from the engine on purpose - `beforeinput` refuses it and `input` only paints a preview - so until it commits the text exists solely in the internal textarea and no `continuity-change` has been emitted for it.
  `destroy()` never folded it in, so a host that persists on the change stream lost whatever was being composed.
  This was not an exotic case: Android keyboards hold one composition open across ordinary typing, so the uncommitted run is routinely the last word the user typed, lost every time the editor unmounted mid-sentence.

  `destroy()` now commits the run before releasing the engine and emits the resulting `continuity-change` (`source: "compositionCommit"`), so the host sees it.
  Unmounting goes through the same path via `disconnectedCallback`.

### Added

- **`commitComposition(): boolean`** on `ContinuityEditorElement`, beside the existing read-only `composing` getter.
  Folds an open IME composition into the engine now and emits its change; returns `false` when no composition was open.
  `destroy()` calls it itself, so teardown needs nothing from the host - this is for checkpointing mid-typing, before a manual save or a navigation.
  Previously a host that knew it was about to unmount had no sanctioned way to fold the run and had to read `[part=input]`.value out of the shadow root.

### Changed

- Package JavaScript budget raised 336 → 352 KiB for the tracked keyboard policy (unminified source ships).

## 0.2.34

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
