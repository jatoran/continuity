// Soft-keyboard state transitions.
//
// The gate that keeps a dismissed Android keyboard down and the gate that would
// dismiss a raised one are the same attribute; only the tracked state tells them
// apart. A headless browser has no IME to raise, so the transitions cannot be
// asserted through one - they are asserted here, against the pure policy the
// gate consults, with every input handed in by the test.

import assert from "node:assert/strict";
import test from "node:test";

const moduleUrl = new URL(
  "./node_modules/@continuity-editor/editor/src/soft_keyboard_policy.js",
  import.meta.url,
);
const { KEYBOARD_DOWN, KEYBOARD_RAISED, createSoftKeyboardPolicy } = await import(moduleUrl);

/** Raise the keyboard the way a phone reports it: the visual viewport shrinks. */
function raiseByViewport(policy, { tall = 800, short = 400 } = {}) {
  policy.observeViewportHeight(tall);
  policy.observeViewportHeight(short);
  return policy;
}

test("an untouched editor reports no keyboard", () => {
  const policy = createSoftKeyboardPolicy();
  assert.equal(policy.visibility, KEYBOARD_DOWN);
  assert.equal(policy.isRaised(), false);
  assert.equal(policy.shouldGateForSelection(), true);
});

test("typing intent alone does not claim a keyboard is up", () => {
  // The request is not the observation. Treating it as one would let the very
  // first selection gesture after a tap decide the keyboard was up and refuse
  // to gate - which is how a dismissed keyboard came back over the selection.
  const policy = createSoftKeyboardPolicy();
  policy.noteTypingIntent();
  assert.equal(policy.hasTypingIntent, true);
  assert.equal(policy.visibility, KEYBOARD_DOWN);
  assert.equal(policy.shouldGateForSelection(), true);
});

test("viewport occlusion is what raises the tracked keyboard", () => {
  const policy = createSoftKeyboardPolicy();
  raiseByViewport(policy);
  assert.equal(policy.visibility, KEYBOARD_RAISED);
  assert.equal(policy.isRaised(), true);
  assert.equal(policy.isViewportObserved, true);
});

test("a selection gesture must not close the gate while a keyboard is up", () => {
  const policy = createSoftKeyboardPolicy();
  raiseByViewport(policy);
  assert.equal(policy.shouldGateForSelection(), false);
});

test("a viewport that merely loses a toolbar is not a keyboard", () => {
  const policy = createSoftKeyboardPolicy();
  policy.observeViewportHeight(800);
  policy.observeViewportHeight(744);
  assert.equal(policy.visibility, KEYBOARD_DOWN);
  assert.equal(policy.isViewportObserved, false);
});

test("the viewport coming back is the back gesture, which blurs nothing", () => {
  const policy = createSoftKeyboardPolicy();
  raiseByViewport(policy);
  assert.equal(policy.observeViewportHeight(800), KEYBOARD_DOWN);
  assert.equal(policy.hasTypingIntent, false);
  // The whole point of the original gate: from here a long-press must gate.
  assert.equal(policy.shouldGateForSelection(), true);
});

test("a proven viewport lets typing intent raise optimistically", () => {
  // Between asking for the IME and the viewport reporting it, a long-press would
  // otherwise read a stale "down" and dismiss a keyboard on its way up.
  const policy = createSoftKeyboardPolicy();
  raiseByViewport(policy);
  policy.observeViewportHeight(800);
  assert.equal(policy.visibility, KEYBOARD_DOWN);
  policy.noteTypingIntent();
  assert.equal(policy.visibility, KEYBOARD_RAISED);
});

test("an optimistic raise the keyboard never honoured is corrected", () => {
  const policy = createSoftKeyboardPolicy();
  raiseByViewport(policy);
  policy.observeViewportHeight(800);
  policy.noteTypingIntent();
  assert.equal(policy.visibility, KEYBOARD_RAISED);
  assert.equal(policy.observeViewportHeight(800), KEYBOARD_DOWN);
});

test("a blur dismisses the keyboard whatever the viewport last said", () => {
  const policy = createSoftKeyboardPolicy();
  raiseByViewport(policy);
  assert.equal(policy.noteDismissed(), KEYBOARD_DOWN);
  assert.equal(policy.hasTypingIntent, false);
  assert.equal(policy.shouldGateForSelection(), true);
});

test("nonsense viewport heights leave the state alone", () => {
  const policy = createSoftKeyboardPolicy();
  raiseByViewport(policy);
  for (const height of [0, -1, Number.NaN, Number.POSITIVE_INFINITY, undefined]) {
    assert.equal(policy.observeViewportHeight(height), KEYBOARD_RAISED, `height ${height}`);
  }
});

test("a keyboard already up when the editor mounts is still detected", () => {
  // There is no absolute "unoccluded" height to compare against, so the tallest
  // height ever seen is the baseline; the keyboard is found when it goes away
  // and the viewport grows past what it had been reporting.
  const policy = createSoftKeyboardPolicy();
  policy.observeViewportHeight(400);
  assert.equal(policy.visibility, KEYBOARD_DOWN);
  policy.observeViewportHeight(800);
  policy.observeViewportHeight(400);
  assert.equal(policy.visibility, KEYBOARD_RAISED);
});

test("a keyboard-down selection grants exactly one raise-only tap", () => {
  const policy = createSoftKeyboardPolicy();
  assert.equal(policy.claimSelectionPreservation("4:9:forward"), true);
  assert.equal(policy.preservedSelectionKey, "4:9:forward");
  // The tap after it has to mean what a tap normally means, or a reader could
  // never tap their way out of a selection they no longer want.
  assert.equal(policy.claimSelectionPreservation("4:9:forward"), false);
});

test("a different selection earns its own raise-only tap", () => {
  const policy = createSoftKeyboardPolicy();
  policy.claimSelectionPreservation("4:9:forward");
  assert.equal(policy.claimSelectionPreservation("11:16:forward"), true);
});

test("a fresh selection gesture restores the grant for the same range", () => {
  const policy = createSoftKeyboardPolicy();
  policy.claimSelectionPreservation("4:9:forward");
  policy.releaseSelectionPreservation();
  assert.equal(policy.claimSelectionPreservation("4:9:forward"), true);
});

test("no raise-only tap is granted while a keyboard is already up", () => {
  // There is nothing to raise, so the tap is simply a tap.
  const policy = createSoftKeyboardPolicy();
  raiseByViewport(policy);
  assert.equal(policy.claimSelectionPreservation("4:9:forward"), false);
});

test("a dismissal returns the grant to the selection that spent it", () => {
  const policy = createSoftKeyboardPolicy();
  policy.claimSelectionPreservation("4:9:forward");
  policy.noteDismissed();
  assert.equal(policy.claimSelectionPreservation("4:9:forward"), true);
});

test("an unusable selection key is never claimable", () => {
  const policy = createSoftKeyboardPolicy();
  for (const key of ["", null, undefined, 7]) {
    assert.equal(policy.claimSelectionPreservation(key), false, `key ${key}`);
  }
});

test("the occlusion threshold is configurable for other form factors", () => {
  const policy = createSoftKeyboardPolicy({ occlusionPx: 40 });
  policy.observeViewportHeight(800);
  assert.equal(policy.observeViewportHeight(744), KEYBOARD_RAISED);
});
