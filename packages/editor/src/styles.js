export const EDITOR_STYLES = `
:host {
  --continuity-background: #111318;
  --continuity-foreground: #e7e9ee;
  --continuity-muted: #8d96a8;
  --continuity-accent: #7aa2f7;
  --continuity-selection: rgb(68 110 180 / 42%);
  --continuity-border: #2b303b;
  --continuity-indent-guide: rgb(141 150 168 / 26%);
  --continuity-indent-guide-active: rgb(122 162 247 / 62%);
  --continuity-decoration: rgb(226 178 76 / 34%);
  --continuity-decoration-active: rgb(226 178 76 / 62%);
  display: block;
  contain: layout paint style;
  min-block-size: 12rem;
  color-scheme: dark;
}
:host([theme="light"]) {
  --continuity-background: #fbfbfc;
  --continuity-foreground: #20232a;
  --continuity-muted: #687083;
  --continuity-accent: #315fba;
  --continuity-selection: rgb(70 120 210 / 30%);
  --continuity-border: #d9dce3;
  --continuity-indent-guide: rgb(104 112 131 / 26%);
  --continuity-indent-guide-active: rgb(49 95 186 / 58%);
  --continuity-decoration: rgb(214 152 26 / 32%);
  --continuity-decoration-active: rgb(214 152 26 / 58%);
  color-scheme: light;
}
@media (prefers-color-scheme: light) {
  :host(:not([theme="dark"])) {
    --continuity-background: #fbfbfc;
    --continuity-foreground: #20232a;
    --continuity-muted: #687083;
    --continuity-accent: #315fba;
    --continuity-selection: rgb(70 120 210 / 30%);
    --continuity-border: #d9dce3;
    --continuity-indent-guide: rgb(104 112 131 / 26%);
    --continuity-indent-guide-active: rgb(49 95 186 / 58%);
    --continuity-decoration: rgb(214 152 26 / 32%);
    --continuity-decoration-active: rgb(214 152 26 / 58%);
    color-scheme: light;
  }
}
.frame {
  position: relative;
  overflow: hidden;
  box-sizing: border-box;
  width: 100%;
  height: 100%;
  min-height: inherit;
  border: 1px solid var(--continuity-border);
  border-radius: 8px;
  background: var(--continuity-background);
  color: var(--continuity-foreground);
}
.projection,
.input {
  box-sizing: border-box;
  margin: 0;
  padding: 1rem 1.1rem;
  width: 100%;
  min-height: 100%;
  font-family: var(--continuity-font-family, ui-monospace, "Cascadia Mono", Consolas, monospace);
  font-size: var(--continuity-font-size, 16px);
  font-style: normal;
  font-weight: 400;
  line-height: var(--continuity-line-height, 1.55);
  tab-size: 4;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
.projection {
  position: absolute;
  inset: 0;
  width: calc(100% - 14px);
  height: max-content;
  min-height: 100%;
  pointer-events: none;
  transform-origin: top left;
  will-change: transform;
}
.affordances {
  position: absolute;
  inset: 0;
  width: calc(100% - 14px);
  height: max-content;
  min-height: 100%;
  pointer-events: none;
  transform-origin: top left;
  will-change: transform;
  z-index: 3;
}
.input {
  position: absolute;
  inset: 0;
  resize: none;
  border: 0;
  outline: 0;
  background: transparent;
  color: transparent;
  caret-color: transparent;
  -webkit-text-fill-color: transparent;
  overflow: auto;
  z-index: 1;
}
.input::selection {
  background: transparent;
  color: transparent;
}
/* Touch surface. A user-select of none is ignored on an editable element, and
   neither selectstart nor contextmenu can refuse the platform's long-press
   selection there, so on touch the finger is kept off the textarea entirely and
   put on this plain div. A non-editable element does honour user-select none,
   which leaves the platform with nothing to select and the projection-owned
   gesture in sole control. The shield also owns scrolling: its spacer is sized
   to the projection, so the scrollable extent is by construction the height of
   what the reader can actually see. */
.touch-shield {
  position: absolute;
  inset: 0;
  z-index: 2;
  overflow: hidden;
  pointer-events: none;
  overscroll-behavior: contain;
}
.touch-shield-spacer { width: 1px; }
/* Replaces the platform selection bubble the shield displaces. Sits above every
   other layer so it stays tappable over the shield. */
.selection-actions {
  position: absolute;
  inset: 0 auto auto 0;
  z-index: 6;
  display: flex;
  gap: 2px;
  padding: 3px;
  border: 1px solid var(--continuity-border);
  border-radius: 8px;
  background: color-mix(in srgb, var(--continuity-background) 92%, var(--continuity-foreground));
  box-shadow: 0 2px 10px rgb(0 0 0 / 28%);
}
.selection-actions[hidden] { display: none; }
.selection-actions-button {
  min-height: 40px;
  padding: 0 12px;
  border: 0;
  border-radius: 6px;
  background: transparent;
  color: var(--continuity-foreground);
  font: 600 14px/1 system-ui, sans-serif;
  cursor: pointer;
}
.selection-actions-button:active {
  background: color-mix(in srgb, var(--continuity-foreground) 16%, transparent);
}
.selection-actions-button[hidden] { display: none; }
.frame.touch-scrolling .touch-shield {
  pointer-events: auto;
  overflow-y: auto;
  overflow-x: hidden;
  -webkit-user-select: none;
  user-select: none;
  -webkit-touch-callout: none;
}
/* A mouse keeps addressing the textarea directly, so pointer behaviour on a
   fine pointer is exactly what it was. */
.frame.touch-scrolling .input {
  pointer-events: none;
  overflow: hidden;
}
.frame:focus-within {
  box-shadow: inset 0 0 0 1px var(--continuity-accent);
}
.keyboard-help {
  position: absolute;
  width: 1px;
  height: 1px;
  padding: 0;
  margin: -1px;
  overflow: hidden;
  clip: rect(0, 0, 0, 0);
  white-space: nowrap;
  border: 0;
}
.composition-run {
  text-decoration: underline;
  text-decoration-skip-ink: none;
}
.frame.composing-fallback .projection { opacity: 0; }
.frame.composing-fallback .input {
  color: var(--continuity-foreground);
  caret-color: var(--continuity-accent);
  -webkit-text-fill-color: var(--continuity-foreground);
}
.frame.composing-fallback .input::selection { background: var(--continuity-selection); }
.frame.composing-fallback .visual-caret,
.frame.composing-fallback .visual-selection { display: none; }
.line {
  box-sizing: border-box;
  min-height: var(--continuity-line-height, 1.55em);
  line-height: var(--continuity-line-height, 1.55em);
  padding-inline-start: var(--continuity-wrap-indent, 0px);
  text-indent: calc(-1 * var(--continuity-wrap-indent, 0px));
}
/* The hanging indent above is expressed as inline padding plus a negative
   first-line indent, which CSS resolves in two different coordinate systems:
   tab stops are anchored at the *content* edge, so the padding pushes the whole
   tab grid right by the indent while the negative indent pulls the first row
   left by it. A tab-indented line's own content therefore renders at
   (indent mod tab-width) while its wrapped rows hang at the full indent —
   visible on every nested bullet, invisible on space-indented ones. Hanging
   inverts
   which rows are indented instead, so the line box needs no padding, the grid
   origin stays under the first row, and both agree. The padding form is left as
   the fallback for engines without the keyword: wrong by a fraction of a tab is
   still better than no hanging indent at all. */
@supports (text-indent: 1px hanging) {
  .line {
    padding-inline-start: 0;
    text-indent: var(--continuity-wrap-indent, 0px) hanging;
  }
}
.line[data-break-unbroken="true"] { word-break: break-all; }
/* Guides paint into the line's own background, whose positioning area is the
   padding box — unmoved by the per-line inline padding the fallback applies. */
.projection[data-indent-guides="on"] .line {
  background-image: var(--continuity-line-guides, none);
  background-repeat: no-repeat;
}
.block-heading-1, .block-heading-2, .block-heading-3,
.block-heading-4, .block-heading-5, .block-heading-6,
.block-setextHeading {
  color: var(--continuity-foreground);
  font-weight: 750;
}
.block-heading-1:not([data-source-visible="true"]) { font-size: 1.45em; }
.block-heading-2:not([data-source-visible="true"]),
.block-setextHeading:not([data-source-visible="true"]) { font-size: 1.32em; }
.block-heading-3:not([data-source-visible="true"]) { font-size: 1.22em; }
.block-heading-4:not([data-source-visible="true"]) { font-size: 1.14em; }
.block-heading-5:not([data-source-visible="true"]) { font-size: 1.08em; }
.block-heading-6:not([data-source-visible="true"]) { font-size: 1.03em; }
.block-blockQuote { border-inline-start: 3px solid var(--continuity-border); padding-inline-start: .75rem; color: var(--continuity-muted); }
/* A thematic break projects to an empty display line — its dashes are a hidden
   marker — so without a rule of its own it read as a blank line. Drawn as a
   pseudo-element rather than a background so it cannot collide with the indent
   guides, and suppressed on the caret's line the way heading sizing is, which
   is what lets the raw source come back under the caret. */
.block-horizontalRule[data-detailed="true"]:not([data-source-visible="true"]) { position: relative; }
.block-horizontalRule[data-detailed="true"]:not([data-source-visible="true"])::after {
  content: "";
  position: absolute;
  inset-inline: 0;
  top: calc(50% - 1px);
  height: 2px;
  border-radius: 1px;
  background: var(--continuity-border);
  pointer-events: none;
}
.block-fencedCodeBlock, .block-indentedCodeBlock { background: color-mix(in srgb, var(--continuity-foreground) 6%, transparent); }
.inline-strong { font-weight: 750; }
.inline-emphasis { font-style: italic; }
.inline-strikethrough { text-decoration: line-through; }
.inline-code { color: var(--continuity-accent); }
.inline-checkbox { color: var(--continuity-accent); }
.inline-link { color: var(--continuity-accent); text-decoration: underline; }
.secondary-carets {
  position: absolute;
  inset: 0;
  pointer-events: none;
  z-index: 2;
}
.visual-caret {
  position: absolute;
  inset: 0 auto auto 0;
  width: 2px;
  background: var(--continuity-accent);
}
.visual-selection {
  position: absolute;
  inset: 0 auto auto 0;
  border-radius: 1px;
  background: var(--continuity-selection);
}
/* Host range decorations. Painted in the same pass as the selection and below
   it, so a decorated range under the caret still reads as selected. The colour
   resolves per set: the rect carries an inline
   \`var(--continuity-decoration-<id>, var(--continuity-decoration))\`, so a host
   themes one set by defining the id-suffixed property and every other set keeps
   the default. */
.decoration {
  position: absolute;
  inset: 0 auto auto 0;
  border-radius: 2px;
  background: var(--continuity-decoration);
  pointer-events: none;
}
/* Touch selection adjust handles. The shield displaces the platform's own, and
   a selection that cannot be nudged after the long-press that drew it is a
   selection the reader has to redo from scratch. Fine pointers keep the drag
   they already have, so the handles are coarse-pointer chrome only. */
.selection-handles {
  position: absolute;
  inset: 0;
  z-index: 5;
  pointer-events: none;
  display: none;
}
.frame.touch-scrolling .selection-handles { display: block; }
.selection-handle {
  position: absolute;
  inset: 0 auto auto 0;
  width: 44px;
  pointer-events: auto;
  touch-action: none;
  -webkit-tap-highlight-color: transparent;
}
.selection-handle[hidden] { display: none; }
.selection-handle::before {
  content: "";
  position: absolute;
  left: 21px;
  top: 0;
  width: 2px;
  height: var(--continuity-handle-stem, 1em);
  background: var(--continuity-accent);
}
.selection-handle::after {
  content: "";
  position: absolute;
  left: 15px;
  top: var(--continuity-handle-stem, 1em);
  width: 14px;
  height: 14px;
  border-radius: 50%;
  background: var(--continuity-accent);
  box-shadow: 0 1px 3px rgb(0 0 0 / 35%);
}
.primary-caret { animation: continuity-caret-blink 1s step-end infinite; }
.frame:not(:focus-within) .primary-caret { display: none; }
@keyframes continuity-caret-blink { 50% { opacity: 0; } }
.code-copy {
  position: absolute;
  box-sizing: border-box;
  min-width: 56px;
  padding: 4px 8px;
  border: 1px solid var(--continuity-border);
  border-radius: 5px;
  opacity: 0;
  pointer-events: none;
  background: color-mix(in srgb, var(--continuity-background) 88%, var(--continuity-foreground));
  color: var(--continuity-foreground);
  font: 600 12px/1.3 system-ui, sans-serif;
  cursor: pointer;
}
.code-copy.visible {
  opacity: 1;
  pointer-events: auto;
}
@media (pointer: coarse) {
  .code-copy { min-width: 64px; min-height: 40px; }
}
.code-copy.copied {
  border-color: var(--continuity-accent);
  background: var(--continuity-accent);
  color: var(--continuity-background);
}
.code-copy-block { right: 23px; }
.code-copy-inline { transform: translateY(-2px); }
/* Rail chrome is sized in px (host-tunable via custom properties): rem would
   resolve against the embedding page's root font-size, which dense hosts set
   as low as 11px — shrinking touch targets far below the 44-48px guideline. */
.command-rail {
  position: absolute;
  inset: auto 0 0 0;
  display: flex;
  align-items: center;
  gap: 3px;
  box-sizing: border-box;
  height: var(--continuity-rail-height, 56px);
  padding: 4px 6px;
  border-top: 1px solid var(--continuity-border);
  background: var(--continuity-background);
  z-index: 4;
}
.command-rail[hidden] { display: none; }
/* The rail floats over the bottom of the frame, so every scrolling layer has to
   end above it or its final rows are unreachable behind the rail. */
.frame.command-rail-active .touch-shield {
  bottom: var(--continuity-rail-height, 56px);
}
.frame.command-rail-active .input {
  bottom: var(--continuity-rail-height, 56px);
  /* The min-height above would otherwise hold the textarea at the full frame
     height regardless of this inset, leaving its last rows behind the rail
     with no scroll range able to bring them out. */
  min-height: 0;
}
.command-rail-buttons {
  display: flex;
  flex: 1;
  gap: 3px;
  overflow-x: auto;
  scrollbar-width: none;
}
.command-rail-buttons::-webkit-scrollbar { display: none; }
.command-rail-button {
  flex: 0 0 auto;
  box-sizing: border-box;
  min-width: var(--continuity-rail-button-size, 48px);
  height: var(--continuity-rail-button-size, 48px);
  padding: 0 6px;
  border: 0;
  border-radius: 6px;
  background: transparent;
  color: var(--continuity-foreground);
  font: 600 var(--continuity-rail-font-size, 17px)/1 system-ui, sans-serif;
  cursor: pointer;
}
.command-rail-button:active {
  background: color-mix(in srgb, var(--continuity-foreground) 14%, transparent);
}
/* A host action can declare an enablement predicate; the button stays in place
   rather than disappearing, so the rail's arrangement never shifts under a
   finger already on its way down. */
.command-rail-button:disabled { opacity: .35; cursor: default; }
.command-rail-glyph { display: inline-flex; align-items: center; justify-content: center; }
.command-rail-glyph > svg, .command-rail-glyph > img {
  width: 1.15em;
  height: 1.15em;
  fill: currentColor;
}
.command-rail-glyph-bold { font-weight: 800; }
.command-rail-glyph-italic { font-style: italic; }
.command-rail-glyph-strikethrough { text-decoration: line-through; }
.command-rail-glyph-inline-code {
  font-family: inherit;
  font-size: 13px;
  color: var(--continuity-accent);
}
.command-rail-settings {
  position: absolute;
  right: 5px;
  bottom: calc(var(--continuity-rail-height, 56px) + 4px);
  z-index: 5;
  box-sizing: border-box;
  width: min(340px, calc(100% - 10px));
  max-height: min(60%, 380px);
  overflow-y: auto;
  padding: 6px;
  border: 1px solid var(--continuity-border);
  border-radius: 8px;
  background: var(--continuity-background);
  color: var(--continuity-foreground);
  font: 500 14px/1.35 system-ui, sans-serif;
}
.command-rail-settings-row {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 2px 0;
}
.command-rail-settings-name { flex: 1; }
.command-rail-settings-toggle,
.command-rail-settings-move,
.command-rail-settings-action {
  box-sizing: border-box;
  min-width: 44px;
  height: 44px;
  border: 1px solid var(--continuity-border);
  border-radius: 5px;
  background: transparent;
  color: var(--continuity-foreground);
  font: 600 14px/1 system-ui, sans-serif;
  cursor: pointer;
}
.command-rail-settings-toggle[aria-pressed="true"] {
  border-color: var(--continuity-accent);
  color: var(--continuity-accent);
}
.command-rail-settings-move:disabled { opacity: .35; cursor: default; }
.command-rail-settings-footer {
  display: flex;
  justify-content: flex-end;
  padding-top: 5px;
}
.command-rail-settings-action { min-width: 64px; }
:host([readonly]) .input { cursor: default; }
@media (prefers-reduced-motion: reduce) {
  *, *::before, *::after { scroll-behavior: auto !important; transition-duration: 0s !important; }
  .primary-caret { animation: none; }
}
`;
