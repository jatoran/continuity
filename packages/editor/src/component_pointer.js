import {
  applyPrimaryPointerCapture,
  applyProjectedPointerSelection,
  handleUnmodifiedPointerClick,
} from "./pointer_gesture.js";
import { applySoftKeyboardGate, raiseSoftKeyboard } from "./soft_keyboard.js";
import { revealCodeAffordanceAt } from "./code_affordances.js";
import { findMarkdownLink } from "./projection.js";
import { addPointerCaret, addVerticalCaret, verticalCaretSelections } from "./multi_selection.js";
import { renderSelectionOverlays } from "./selection_overlays.js";
import { selectionFromInput } from "./coordinates.js";
import { inputSelectionKey } from "./input_sync.js";

// Pointer and projected-selection application for the editor element. These
// operate on a `ctx` bag the element builds once (stable DOM refs plus small
// closures over its private state), so the class body stays under the file cap
// while this file owns one responsibility: turning a physical pointer gesture
// into an engine + textarea + overlay selection.

/**
 * Whether a pointer event began on the editing surface. Pointer listeners sit
 * on the frame so both the textarea and the touch shield feed them, but the
 * frame also holds the command rail and the code affordances — events from
 * those are chrome, not gestures.
 */
export function isSurfaceEvent(ctx, event) {
  return event.target === ctx.input
    || event.target === ctx.shield
    || Boolean(ctx.shield?.contains(event.target));
}

/**
 * Refuse the scroll while a selection drag owns the finger.
 *
 * The shield is a real scroller, so as soon as the finger moves the browser
 * starts panning it and cancels the pointer — which ends the drag on its first
 * millimetre. `touch-action` is evaluated at touchstart and cannot be changed
 * once the gesture is under way, so the only lever left is preventing the
 * default on `touchmove` itself, which requires a non-passive listener. The
 * press matures while the finger is stationary, so no scroll has begun yet and
 * the browser still honours it.
 */
export function handleTouchMove(ctx, event) {
  if (ctx.touchSelection.isActive && event.cancelable) event.preventDefault();
}

/** Run a gesture handler only for events that began on the editing surface. */
export function routeSurfacePointer(ctx, event, handler) {
  if (isSurfaceEvent(ctx, event)) handler(ctx, event);
}

/**
 * Freeze the element's stable DOM refs and state accessors into one bag. Every
 * function in this file reads from it, which is what keeps the element's class
 * body under the file cap while the gesture logic lives here.
 */
export function createPointerContext(context) {
  return context;
}

/** Begin a projection-owned gesture; a fresh gesture drops any deferred tap. */
export function handlePointerDown(ctx, event) {
  if (!ctx.editor() || event.button !== 0) return;
  // A fresh gesture supersedes any tap still deferred behind composition.
  ctx.setPending(null);
  applyPrimaryPointerCapture(ctx.input, event, ctx.surface?.() ?? ctx.input);
  if (!event.ctrlKey && !event.metaKey) ctx.setCollapse(true);
  ctx.gesture.begin(event, ctx.projection, ctx.editor().snapshot().selections);
  ctx.touchSelection.begin(event, (point) => applyLongPressSelection(ctx, point));
}

/**
 * Select the word under a matured long-press through the projection. Driven by
 * the gesture's own timer because Android Chrome raises no `contextmenu` for a
 * long-press on a textarea; it opens the platform selection UI instead.
 */
function applyLongPressSelection(ctx, point) {
  if (!ctx.editor()) return false;
  // Refusing to claim while composing disabled long-press selection outright on
  // Android, where the IME keeps a composition open across the whole gesture.
  // Commit the composing run instead: the engine then matches the textarea and
  // the projected position maps exactly.
  if (ctx.isComposing()) ctx.commitComposition();
  const position = ctx.touchSelection.claimAt(point, ctx.projection);
  if (!position) return false;
  // This gesture is selection, not typing. Close the keyboard gate now, while
  // the finger is still down: the platform may synthesize a trailing tap when it
  // lifts, and a tap against the still-focused textarea is all Chrome needs to
  // raise the IME over the words being selected.
  applySoftKeyboardGate(ctx.input);
  commitProjectedSelection(ctx, { anchor: position, head: position });
  ctx.executeCommand("editor.select_word");
  ctx.sync();
  const selected = ctx.editor().snapshot().selections[0];
  if (selected) {
    // Anchor on the whole word, not one of its ends, so a finger resting inside
    // it keeps it selected and drags grow from whichever edge it passes.
    const isForward = comparePositions(selected.anchor, selected.head) <= 0;
    ctx.touchSelection.anchorToRange(
      isForward ? selected.anchor : selected.head,
      isForward ? selected.head : selected.anchor,
    );
  }
  // Own the pointer for the rest of the gesture. Without capture the platform
  // can retarget the touch and cancel it mid-drag, which ends the selection
  // early and reads as "it will not let me drag any further".
  const surface = ctx.surface?.() ?? ctx.input;
  if (surface.setPointerCapture && ctx.touchSelection.pointerId !== undefined) {
    try {
      surface.setPointerCapture(ctx.touchSelection.pointerId);
    } catch {
      // The pointer may already be gone; the gesture simply ends.
    }
  }
  renderSelectionOverlays(ctx.carets, ctx.projection, ctx.editor().snapshot());
  ctx.schedule();
  return true;
}

/** Reveal hovered code affordances and commit any mouse/pen drag selection. */
export function handlePointerMove(ctx, event) {
  revealCodeAffordanceAt(ctx.affordances, event.clientX, event.clientY);
  // A claimed long-press owns the finger; extend it against projected geometry
  // rather than letting the platform re-select against the textarea.
  ctx.touchSelection.disarmIfWandered(event);
  if (ctx.touchSelection.isActive) {
    extendTouchSelectionAt(ctx, event, event.clientX, event.clientY);
    // The drag refuses the scroll, so the finger cannot reach past the viewport
    // on its own; scroll for it while it rests near an edge.
    ctx.autoScroller.update(
      ctx.scroller(), event.clientX, event.clientY,
      (point) => extendTouchSelectionAt(ctx, null, point.clientX, point.clientY),
    );
    return;
  }
  commitProjectedSelection(ctx, ctx.gesture.move(event, ctx.projection));
}

/**
 * Extend a claimed selection to one point and repaint immediately. Painting on
 * the event rather than the next scheduled frame keeps the highlight under the
 * finger instead of trailing it.
 */
function extendTouchSelectionAt(ctx, event, clientX, clientY) {
  const extended = ctx.touchSelection.extend(
    event ?? { clientX, clientY, cancelable: false }, ctx.projection,
  );
  if (!extended) return;
  commitProjectedSelection(ctx, extended);
  renderSelectionOverlays(ctx.carets, ctx.projection, ctx.editor().snapshot());
}

/** A scroll or platform gesture took the pointer; drop all tap/drag state. */
export function handlePointerCancel(ctx) {
  ctx.gesture.cancel();
  releaseTouchSelection(ctx);
  ctx.setCollapse(false);
  ctx.setPending(null);
}

/** Release a claimed long-press once the finger lifts. */
export function handlePointerUp(ctx) {
  // Read before releasing: `cancel()` clears the gesture, and whether it ever
  // matured decides if a bare caret gets the bar.
  const wasLongPress = ctx.touchSelection.hasClaimed;
  releaseTouchSelection(ctx);
  // A long-press is already resolved as selection, never as a later tap. Some
  // engines still synthesize a click after the pointer lifts; leave it no tap
  // state to collapse the range or open the soft keyboard.
  if (wasLongPress) ctx.gesture.cancel();
  // Offered only once the gesture settles, so it never sits under the finger
  // that is still drawing the selection.
  if (wasLongPress) ctx.offerSelectionActionsForCaret?.();
  else ctx.updateSelectionActions?.();
}

/**
 * Run one selection-bar action. Copy and cut read the engine's own selection
 * rather than the textarea's, so they act on exactly what is highlighted.
 */
export async function runSelectionAction(ctx, action, capturedText) {
  const editor = ctx.editor();
  if (!editor) return;
  if (action === "select-all") {
    // TODO(#1): the WASM engine rejects `editor.select_all`, which the Rust
    // command registry does define. Selecting the whole document is an
    // unambiguous range rather than an edit plan, so it is expressed directly
    // here until the command is exposed to every binding.
    const lines = editor.snapshot().text.split("\n");
    const lastLine = lines.length - 1;
    editor.setSelections([{
      anchor: { line: 0, byteInLine: 0 },
      head: { line: lastLine, byteInLine: new TextEncoder().encode(lines[lastLine]).byteLength },
      kind: "caret",
    }]);
    ctx.sync();
    ctx.schedule();
    ctx.updateSelectionActions?.();
    return;
  }
  if (action === "paste") {
    // Read before anything can consume the user activation the browser requires
    // for a clipboard read.
    const text = await ctx.readClipboard();
    ctx.hideSelectionActions?.();
    if (typeof text !== "string" || text.length === 0) return;
    // The tap moved focus to the button; the insert has to go to the editor.
    // Paste is typing intent expressed through a button rather than a tap, so it
    // lifts the keyboard gate for the same reason a resolved tap does.
    raiseSoftKeyboard(ctx.input);
    ctx.insertText(text);
    return;
  }
  // Prefer the text captured when the button went down: by the time the action
  // runs, the tap may already have collapsed the textarea's selection.
  const { input } = ctx;
  const selected = capturedText ?? input.value.slice(input.selectionStart, input.selectionEnd);
  if (selected.length === 0) return;
  await ctx.writeClipboard(selected);
  if (action === "cut") ctx.insertText("");
  ctx.hideSelectionActions?.();
}

function releaseTouchSelection(ctx) {
  ctx.autoScroller.stop();
  const pointerId = ctx.touchSelection.pointerId;
  const surface = ctx.surface?.() ?? ctx.input;
  if (pointerId !== undefined && surface.hasPointerCapture?.(pointerId)) {
    surface.releasePointerCapture(pointerId);
  }
  ctx.touchSelection.cancel();
}

/**
 * Take over the platform's long-press selection for touch. `contextmenu` is
 * raised at the platform's own long-press threshold and is the only point at
 * which its selection UI can be suppressed; claiming it here keeps selection on
 * projected geometry, which is the only geometry the reader can see.
 */
export function handleTouchLongPress(ctx, event) {
  // The gesture timer normally claims first. This covers engines that do raise
  // `contextmenu` for a touch long-press, and suppresses the menu either way.
  if (!ctx.touchSelection.isPointerDown) return false;
  if (ctx.touchSelection.isActive) {
    event.preventDefault();
    return true;
  }
  const claimed = applyLongPressSelection(ctx, {
    clientX: event.clientX, clientY: event.clientY,
  });
  if (claimed) event.preventDefault();
  return claimed;
}

/**
 * Suppress the platform's own selection once this gesture owns the finger.
 * `selectstart` is where the platform commits to selecting against the
 * textarea; preventing it leaves the caret, soft keyboard, and paste
 * affordances intact, unlike a blanket `user-select: none`.
 */
export function handleSelectStart(ctx, event) {
  if (ctx.touchSelection.isActive) event.preventDefault();
}

/** Document order of two source positions. */
function comparePositions(left, right) {
  return left.line - right.line || left.byteInLine - right.byteInLine;
}

/**
 * Adopt the textarea's own caret into the engine. This is the platform's
 * answer, hit-tested against the invisible textarea rather than the projection,
 * so it is a fallback and never a preference — but it is always where typing
 * will land, which makes it strictly better than drawing a stale caret.
 */
export function adoptNativeCaret(ctx) {
  const editor = ctx.editor();
  if (!editor) return;
  const key = inputSelectionKey(ctx.input);
  if (key === ctx.getSelectionKey?.()) return;
  const text = editor.snapshot().text;
  // A live composition leaves the textarea holding more text than the engine,
  // so a textarea offset would map through engine text off by the composed run.
  // Only adopt while the two buffers agree; otherwise the deferred projected
  // position replayed at compositionEnd remains the canonical answer.
  if (text.length !== ctx.input.value.length) return;
  editor.setSelections([selectionFromInput(text, ctx.input)]);
  ctx.setSelectionKey(key);
  ctx.schedule();
}

/** Commit one projection-mapped range to the engine, textarea, and render. */
export function commitProjectedSelection(ctx, selection) {
  const selectionKey = applyProjectedPointerSelection(ctx.editor(), ctx.input, selection);
  if (selectionKey !== null) {
    ctx.setSelectionKey(selectionKey);
    ctx.schedule();
  }
}

/**
 * Replay a tap that landed during composition against the fresh snapshot. The
 * engine only matches the textarea after `compositionend` reconciles, so the
 * intended canonical position is retained until then and applied here.
 */
export function applyPendingProjectedSelection(ctx) {
  const pending = ctx.getPending();
  ctx.setPending(null);
  if (!pending || !ctx.editor()) return;
  ctx.setCollapse(false);
  commitProjectedSelection(ctx, pending);
}

/** Complete a primary click into a selection, checkbox toggle, or link open. */
export function completeProjectedPointerClick(ctx, event) {
  const gesture = ctx.gesture.complete(event);
  // Native scrolling normally ends in pointercancel and no click. Keep this
  // defensive boundary for engines that deliver a trailing click after travel
  // or cancellation: only a resolved touch tap may mutate selection or focus.
  if (gesture.pointerType === "touch" && !gesture.isTap) {
    ctx.setCollapse(false);
    return;
  }
  if (ctx.isComposing()) {
    // In the whole-frame fallback the projection is not rendered, so the
    // textarea's own caret is the only answer available.
    if (ctx.frame.classList.contains("composing-fallback")) {
      adoptNativeCaret(ctx);
      return;
    }
    // Otherwise commit the composing run and apply the projected position now.
    // Deferring it until compositionEnd left the caret wherever the platform
    // put it — hit-tested against the invisible textarea, so off by every
    // folded marker above the tap — for as long as the IME kept composing,
    // which on Android is indefinitely.
    ctx.commitComposition();
  }
  if (!gesture.selection) {
    // The projection could not be hit-tested (a tap past the last glyph, on
    // chrome, or on a line with no realized geometry). Doing nothing here left
    // the engine stale while the platform caret moved, so the drawn caret
    // disagreed with where typing went. Fall back to the platform's own answer.
    adoptNativeCaret(ctx);
  }
  commitProjectedSelection(ctx, gesture.selection);
  // This stays in the trusted click turn. Mobile browsers require focus to be
  // synchronous with user activation in order to open the soft keyboard. The
  // projected caret is committed first so the keyboard targets the visible
  // character rather than the hidden textarea's raw-layout answer.
  //
  // A resolved touch tap is the only gesture that means "type here", which makes
  // it the only place the soft-keyboard gate opens. Every other touch — the
  // long-press, the drag that extends it, a handle grab — leaves it closed, so a
  // reader selecting text on a phone keeps the screen they were reading.
  if (gesture.pointerType === "touch" && gesture.isTap) {
    raiseSoftKeyboard(ctx.input);
  }
  if (handleUnmodifiedPointerClick(
    event, gesture, ctx.editor(), ctx.input,
    () => { ctx.sync(); ctx.schedule(); },
    () => ctx.executeCommand("markdown.toggle_checkbox"),
  )) {
    ctx.setCollapse(false);
    return;
  }
  const link = findMarkdownLink(ctx.input.value, ctx.input.selectionStart);
  if (link) {
    ctx.emitRequest("openLink", link);
  } else if (ctx.editor() && gesture.selectionsBeforePointer) {
    ctx.editor().setSelections(addPointerCaret(
      ctx.input.value,
      gesture.selectionsBeforePointer,
      ctx.input,
    ));
    ctx.sync();
    renderSelectionOverlays(ctx.carets, ctx.projection, ctx.editor().snapshot());
    ctx.schedule();
  }
}

/** Add or remove a vertical (column) caret above/below the primary caret. */
export function applyVerticalCaret(ctx, direction) {
  if (!ctx.editor()) return;
  ctx.editor().setSelections(addVerticalCaret(ctx.editor().snapshot(), direction, ctx.input));
  ctx.sync();
  renderSelectionOverlays(ctx.carets, ctx.projection, ctx.editor().snapshot());
  ctx.schedule();
}

/**
 * Move the caret one visible (soft-wrapped) row up or down, collapsing any
 * range. This is the pointerless equivalent of the platform arrow key: the
 * rail's caret actions run it because a rail button must not steal focus from
 * the textarea, so no synthetic key event can be delivered to it.
 */
export function moveVerticalCaret(ctx, direction) {
  if (!ctx.editor()) return;
  // The rail never takes focus, so the textarea may hold a caret the engine has
  // not adopted yet; move from where typing would actually land.
  adoptNativeCaret(ctx);
  ctx.editor().setSelections(verticalCaretSelections(
    ctx.editor().snapshot(), direction, ctx.input,
  ));
  ctx.sync(true);
  ctx.setSelectionKey(inputSelectionKey(ctx.input));
  renderSelectionOverlays(ctx.carets, ctx.projection, ctx.editor().snapshot());
  ctx.schedule();
}

/** Collapse every secondary selection back to the single textarea caret. */
export function clearSecondarySelections(ctx) {
  if (!ctx.editor()) return;
  ctx.editor().setSelections([selectionFromInput(ctx.input.value, ctx.input)]);
  ctx.setSelectionKey(inputSelectionKey(ctx.input));
  ctx.schedule();
}
