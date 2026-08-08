/** Host-replacement deferral policy for active IME composition. */

/**
 * Decide whether a host document replacement that was deferred during an IME
 * composition should be applied now that the composition has committed.
 *
 * `pending` is the coalesced `{ value, revision }` captured while composing, or
 * `null` when nothing was deferred. `snapshot` is the engine state *after* the
 * composition commit. Returns the replacement to apply, or `null` to drop it:
 *
 * - already-matching text is a no-op;
 * - a `replaceValue` echo whose revision no longer matches the engine is a
 *   stale reflection of pre-composition state and is dropped so the freshly
 *   committed composition wins (and so the engine never throws a revision
 *   conflict);
 * - a forced replacement (`revision === null`, from the imperative `value`
 *   setter) re-anchors to the current revision and applies unconditionally.
 */
export function planDeferredHostReplace(pending, snapshot) {
  if (!pending) {
    return null;
  }
  if (snapshot.text === pending.value) {
    return null;
  }
  if (pending.revision != null && pending.revision !== snapshot.revision) {
    return null;
  }
  return { value: pending.value, revision: pending.revision ?? snapshot.revision };
}
