// The coarse-pointer pass.
//
// The main browser contract runs as a desktop pointer, where the whole touch
// input surface is inert by design — the shield does not take the finger, the
// adjust handles never appear, and the soft-keyboard gate is a no-op. So the
// runner re-runs these under touch emulation as a second pass, and this module
// is the list of what that pass covers.
//
// It is kept apart from the main suite because the two grow for different
// reasons: the main suite grows with the editor's contract, this one grows with
// the touch surface, and pooling them is what pushed the suite past the file cap.

import { runTouchShieldTests } from "./browser-touch-shield.mjs";
import { runSelectionHandleTests } from "./browser-projection-chrome.mjs";
import { runSoftKeyboardGateTests } from "./browser-soft-keyboard.mjs";
import { runRangeRevealTests } from "./browser-range-reveal.mjs";

/**
 * Run every coarse-pointer contract and report per-contract assertion counts.
 * The counts are load-bearing: the runner refuses a pass where any of them came
 * back zero, which is what stops an emulation change from silently skipping a
 * whole contract and still reporting green.
 */
export async function runCoarsePointerPass(ContinuityEditorElement) {
  const failures = [];
  let total = 0;
  const record = (condition, message) => {
    total += 1;
    if (!condition) failures.push(message);
  };
  const shieldAssertions = await runTouchShieldTests(ContinuityEditorElement, record);
  const handleAssertions = await runSelectionHandleTests(ContinuityEditorElement, record);
  const softKeyboardAssertions = await runSoftKeyboardGateTests(ContinuityEditorElement, record);
  const rangeRevealAssertions = await runRangeRevealTests(ContinuityEditorElement, record);
  return {
    assertions: shieldAssertions + handleAssertions + softKeyboardAssertions + rangeRevealAssertions,
    shieldAssertions,
    handleAssertions,
    softKeyboardAssertions,
    rangeRevealAssertions,
    total,
    failures,
  };
}
