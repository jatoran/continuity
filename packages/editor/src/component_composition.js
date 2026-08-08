// IME composition lifecycle for the editor element.
//
// Split out because composition is not the brief, exceptional state the rest of
// the element assumes. Android keyboards hold one open continuously across
// ordinary typing, so anything a composition blocks is blocked permanently on a
// phone — and the commit below has to be callable mid-composition, not only at
// `compositionend`, so a projection-owned gesture can put the engine and the
// textarea back onto the same text before mapping offsets between them.

import { clearCompositionPresentation, renderCompositionPresentation } from "./composition_preview.js";
import { clearProjectionCompositionLine } from "./projection.js";
import { planDeferredHostReplace } from "./composition_defer.js";
import { reconcileTextareaMutation } from "./textarea_reconcile.js";
import { applyPendingProjectedSelection } from "./component_pointer.js";

/** Open a composition: the engine stops absorbing text until it commits. */
export function beginComposition(ctx) {
  if (ctx.isReadOnly()) return;
  ctx.setComposing(true);
  ctx.setComposedRunLength(0);
  ctx.setPending(null);
}

/**
 * Close a composition and release whatever it held back.
 *
 * The commit runs first, but the two releases run whether or not it did any
 * work: a tap or a pressed Enter commits the run mid-composition, so by the
 * time the IME closes there is nothing left to fold, while anything deferred
 * behind the composition is still waiting and would otherwise sit until some
 * later composition ended. A tap that landed while the engine still held
 * pre-composition text was retained rather than applied; now that the engine
 * matches the textarea it can be applied canonically. Both releases are no-ops
 * when nothing is pending.
 */
export function endComposition(ctx) {
  commitComposition(ctx);
  if (!ctx.editor()) return;
  applyPendingProjectedSelection(ctx);
  flushDeferredHostReplace(ctx);
}

/** Paint the live composing line and its caret from the textarea. */
export function renderComposingPresentation(ctx) {
  renderCompositionPresentation(
    ctx.frame, ctx.projection, ctx.carets, ctx.input, ctx.composedRunLength(),
  );
}

/**
 * Fold the live composing run into the engine. Returns false when there was no
 * composition to commit, which is how `compositionend` knows to stop.
 */
export function commitComposition(ctx) {
  if (!ctx.isComposing() || !ctx.editor()) return false;
  ctx.setComposing(false);
  clearCompositionPresentation(ctx.frame);
  clearProjectionCompositionLine(ctx.projection);
  const change = reconcileTextareaMutation(ctx.editor(), ctx.input);
  // Textarea already holds the composed text; sync without re-applying edits
  // (which would double it).
  ctx.commitChange(change, "compositionCommit", null);
  return true;
}

/** Apply a host `replaceValue` that arrived while a composition was open. */
export function flushDeferredHostReplace(ctx) {
  const plan = planDeferredHostReplace(ctx.getDeferredReplace(), ctx.editor().snapshot());
  ctx.setDeferredReplace(null);
  if (!plan) return;
  ctx.commitChange(ctx.editor().replaceValue(plan.value, plan.revision), "hostReplacement");
}
