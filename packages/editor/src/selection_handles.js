// Touch selection adjust handles.
//
// The touch shield takes the finger off the textarea, which is the only way a
// selection can be drawn against the geometry the reader actually sees — but it
// also takes away the platform's drag handles. Without a replacement a phone
// selection is whatever the long-press-and-drag produced and cannot be nudged,
// so a reader who overshot by one word starts the gesture again.
//
// The handles are chrome, not a gesture: the edge being dragged moves through
// `projectionPositionAtPoint` — the same mapping the long-press drag uses —
// while the opposite edge is frozen at the value it held when the handle was
// grabbed. Freezing it is what lets a drag cross the anchor and keep going:
// re-deriving "the other end" from the live selection would flip the anchor the
// moment the ends swapped, and the selection would collapse under the finger.

const KNOB_CLEARANCE_PX = 28;
const HANDLE_HALF_WIDTH_PX = 22;

/** Build both handles and return the controller the element drives. */
export function createSelectionHandles(dom, hooks) {
  const layer = document.createElement("div");
  layer.className = "selection-handles";
  layer.setAttribute("aria-hidden", "true");
  const handles = { start: buildHandle("start"), end: buildHandle("end") };
  layer.append(handles.start, handles.end);
  dom.frame.append(layer);

  // Where each handle's edge currently sits, in client space, so a drag can
  // aim at the glyph the handle marks instead of at the finger covering it.
  const placements = { start: null, end: null };
  let draggingEdge = null;
  let grabOffset = { x: 0, y: 0 };

  const controller = {
    element: layer,
    /** Whether a handle currently owns a pointer. */
    get isDragging() {
      return draggingEdge !== null;
    },
    hide() {
      handles.start.hidden = true;
      handles.end.hidden = true;
    },
    /** Re-place both handles against the selection the overlay just painted. */
    update() {
      if (!hooks.isEnabled()) {
        controller.hide();
        return;
      }
      const rects = [...dom.carets.querySelectorAll('.visual-selection[data-selection-index="0"]')]
        .map((rect) => rect.getBoundingClientRect())
        .filter((rect) => rect.height > 0);
      if (rects.length === 0) {
        controller.hide();
        return;
      }
      const frameBounds = dom.frame.getBoundingClientRect();
      const first = rects[0];
      const last = rects.at(-1);
      placements.start = { clientX: first.left, clientY: first.top, height: first.height };
      placements.end = { clientX: last.right, clientY: last.top, height: last.height };
      placeHandle(handles.start, frameBounds, placements.start);
      placeHandle(handles.end, frameBounds, placements.end);
    },
  };

  for (const edge of ["start", "end"]) {
    const handle = handles[edge];
    handle.addEventListener("pointerdown", (event) => {
      if (event.button !== 0 || !placements[edge]) return;
      // The frame's own gesture handling must not also see this: a press here
      // is chrome, and letting it through begins a fresh caret gesture that
      // collapses the very selection being adjusted.
      event.stopPropagation();
      event.preventDefault();
      draggingEdge = edge;
      grabOffset = {
        x: event.clientX - placements[edge].clientX,
        y: event.clientY - (placements[edge].clientY + placements[edge].height / 2),
      };
      try {
        handle.setPointerCapture(event.pointerId);
      } catch {
        // The pointer may already be gone; the drag simply never starts.
      }
      hooks.onDragStart?.(edge);
    });
    handle.addEventListener("pointermove", (event) => {
      if (draggingEdge !== edge) return;
      event.stopPropagation();
      if (event.cancelable) event.preventDefault();
      hooks.extendEdge?.(event.clientX - grabOffset.x, event.clientY - grabOffset.y);
    });
    for (const type of ["pointerup", "pointercancel"]) {
      handle.addEventListener(type, (event) => {
        if (draggingEdge !== edge) return;
        event.stopPropagation();
        draggingEdge = null;
        if (handle.hasPointerCapture?.(event.pointerId)) {
          handle.releasePointerCapture(event.pointerId);
        }
        hooks.onDragEnd?.();
      });
    }
  }

  return controller;
}

function buildHandle(edge) {
  const handle = document.createElement("div");
  handle.className = "selection-handle";
  handle.dataset.selectionEdge = edge;
  handle.setAttribute("part", `selection-handle selection-handle-${edge}`);
  handle.hidden = true;
  return handle;
}

function placeHandle(handle, frameBounds, placement) {
  const left = placement.clientX - frameBounds.left - HANDLE_HALF_WIDTH_PX;
  const top = placement.clientY - frameBounds.top;
  const stem = Math.round(placement.height);
  handle.style.setProperty("--continuity-handle-stem", `${stem}px`);
  handle.style.height = `${stem + KNOB_CLEARANCE_PX}px`;
  handle.style.transform = `translate(${Math.round(left)}px, ${Math.round(top)}px)`;
  handle.hidden = false;
}
