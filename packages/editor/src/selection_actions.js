// Touch selection action bar.
//
// Taking the platform's long-press selection away also takes away the bubble it
// carried — copy, cut, paste, select all. On a phone that is the only way to
// reach the clipboard, so the editor has to supply it. The bar appears once a
// non-collapsed selection settles and the finger lifts, positioned clear of the
// selection rather than under it.

const CLEARANCE_PX = 10;

const ACTIONS = [
  { id: "copy", label: "Copy" },
  { id: "cut", label: "Cut" },
  { id: "paste", label: "Paste" },
  { id: "select-all", label: "Select all" },
];

/** Build the bar and return a controller the element drives. */
export function createSelectionActions(dom, hooks) {
  const bar = document.createElement("div");
  bar.className = "selection-actions";
  bar.setAttribute("role", "toolbar");
  bar.setAttribute("aria-label", "Selection actions");
  bar.hidden = true;
  for (const action of ACTIONS) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "selection-actions-button";
    button.dataset.selectionAction = action.id;
    button.textContent = action.label;
    // Capture the selected text on the way down. Tapping the button can move
    // focus and collapse the textarea's selection, so reading it later can find
    // nothing left to copy — which looks exactly like a dead button.
    let armed = null;
    button.addEventListener("pointerdown", (event) => {
      armed = hooks.captureSelection();
      // Preventing the default here suppresses the synthesized `click` on
      // touch, so only a mouse — where it stops the blur — gets it.
      if (event.pointerType === "mouse") event.preventDefault();
    });
    // `pointerup` and `click` both fire it; whichever arrives first disarms.
    const fire = () => {
      const captured = armed;
      armed = null;
      if (captured !== null) hooks.run(action.id, captured);
    };
    button.addEventListener("pointerup", fire);
    button.addEventListener("click", fire);
    bar.append(button);
  }
  dom.frame.append(bar);

  // A long-press on text with nothing selected still deserves the clipboard:
  // Select all and Paste apply to a bare caret, and on a phone this bar is the
  // only route to either. Copy and Cut hide themselves when there is nothing
  // selected rather than sitting there inert.
  let isOfferedForCaret = false;

  const setVisible = (anchorBounds, selectionExtent, hasSelection) => {
    bar.hidden = false;
    for (const button of bar.querySelectorAll(".selection-actions-button")) {
      const id = button.dataset.selectionAction;
      const needsSelection = id === "copy" || id === "cut";
      button.hidden = (needsSelection && !hasSelection) || (id === "paste" && !hooks.canPaste());
    }
    positionBar(bar, dom.frame, anchorBounds, selectionExtent);
  };

  return {
    element: bar,
    hide() {
      isOfferedForCaret = false;
      if (!bar.hidden) bar.hidden = true;
    },
    /** Offer the bar for a bare caret, as a long-press with no word does. */
    offerForCaret() {
      isOfferedForCaret = true;
      this.update();
    },
    /** Show the bar above the selection, or below it when there is no room. */
    update() {
      if (!hooks.isEnabled()) {
        bar.hidden = true;
        return;
      }
      const rects = primarySelectionRects(dom.carets);
      if (rects.length > 0) {
        setVisible(rects[0], verticalExtentOf(rects), true);
        return;
      }
      const caret = isOfferedForCaret ? dom.carets.querySelector(".visual-caret") : null;
      if (!caret) {
        bar.hidden = true;
        return;
      }
      const caretBounds = caret.getBoundingClientRect();
      setVisible(caretBounds, caretBounds, false);
    },
  };
}

/**
 * Every painted row of the primary selection, in document order.
 *
 * The overlay paints one rectangle per visual row, so a selection spanning
 * several rows is several elements. The bar anchors to the first, where the
 * selection starts, but has to know how far the rest of it reaches before it
 * can decide whether the selection is still on screen at all.
 */
function primarySelectionRects(carets) {
  const highlights = [...carets.querySelectorAll(".visual-selection")];
  const primaryIndex = highlights[0]?.dataset.selectionIndex;
  return highlights
    .filter((element) => element.dataset.selectionIndex === primaryIndex)
    .map((element) => element.getBoundingClientRect());
}

function verticalExtentOf(rects) {
  return {
    top: Math.min(...rects.map((rect) => rect.top)),
    bottom: Math.max(...rects.map((rect) => rect.bottom)),
  };
}

function positionBar(bar, frame, anchorBounds, selectionExtent) {
  const placement = computeSelectionActionsPlacement(
    frame.getBoundingClientRect(),
    bar.getBoundingClientRect(),
    anchorBounds,
    selectionExtent,
  );
  bar.style.transform = `translate(${placement.left}px, ${placement.top}px)`;
}

/**
 * Frame-relative placement for the bar: prefer above the anchor, fall to below
 * it when there is no room, and stay on screen while any part of the selection
 * is.
 *
 * A selection can be taller than the viewport. Placing the bar from the anchor
 * alone then parks it wherever the selection *starts*, so it leaves the screen
 * the moment that start scrolls off, and on a phone this bar is the only route
 * to the clipboard, which makes scrolling back to find it the whole cost of the
 * feature. Clamping into the frame pins it to the visible edge instead, and it
 * follows the scroll from there.
 *
 * `selectionExtent` is the full painted span of the selection, which is why the
 * clamp survives the start scrolling away and is released only once the whole
 * selection is off screen, past its end as well as its start. With none of it
 * visible the bar has nothing to sit beside, and pinning it would leave a
 * toolbar hovering over text it does not describe.
 *
 * Every unclamped position that is already visible lies inside the clamp range,
 * so a selection that fits on screen keeps exactly the placement it had.
 */
export function computeSelectionActionsPlacement(
  frameBounds, barBounds, anchorBounds, selectionExtent = anchorBounds,
) {
  const above = anchorBounds.top - frameBounds.top - barBounds.height - CLEARANCE_PX;
  const below = anchorBounds.bottom - frameBounds.top + CLEARANCE_PX;
  const left = Math.max(
    CLEARANCE_PX,
    Math.min(
      anchorBounds.left - frameBounds.left,
      frameBounds.width - barBounds.width - CLEARANCE_PX,
    ),
  );
  return {
    left: Math.round(left),
    top: Math.round(clampBarTop(
      above >= 0 ? above : below, frameBounds, barBounds, selectionExtent,
    )),
  };
}

function clampBarTop(top, frameBounds, barBounds, selectionExtent) {
  const isSelectionVisible = selectionExtent.bottom > frameBounds.top
    && selectionExtent.top < frameBounds.bottom;
  if (!isSelectionVisible) return top;
  return Math.min(Math.max(top, 0), Math.max(0, frameBounds.height - barBounds.height));
}
