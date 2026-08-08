import assert from "node:assert/strict";
import test from "node:test";

const moduleUrl = new URL(
  "./node_modules/@continuity-editor/editor/src/selection_actions.js",
  import.meta.url,
);
const { computeSelectionActionsPlacement } = await import(moduleUrl);

// A phone-sized frame with a four-button bar. The clamp range the bar is held
// inside while any of the selection is visible is [0, 600 - 40] = [0, 560].
const FRAME = { top: 100, left: 50, width: 400, height: 600, bottom: 700, right: 450 };
const BAR = { width: 200, height: 40 };

const rect = (top, bottom, left = 90) => ({ top, bottom, left });

test("a selection with room above keeps the preferred placement", () => {
  const placement = computeSelectionActionsPlacement(FRAME, BAR, rect(300, 330));
  assert.deepEqual(placement, { left: 40, top: 150 });
});

test("a selection against the frame top falls below itself", () => {
  const placement = computeSelectionActionsPlacement(FRAME, BAR, rect(110, 140, 60));
  assert.deepEqual(placement, { left: 10, top: 50 });
});

test("the bar never leaves the frame horizontally", () => {
  const placement = computeSelectionActionsPlacement(FRAME, BAR, rect(300, 330, 440));
  assert.equal(placement.left, 190);
});

test("a selection whose start scrolled off the top pins the bar to the frame top", () => {
  // The reported case: the selection spans more than one viewport, so its
  // start is above the frame while the rest of it is still on screen.
  const placement = computeSelectionActionsPlacement(
    FRAME, BAR, rect(-400, -380), { top: -400, bottom: 500 },
  );
  assert.equal(placement.top, 0);
});

test("a selection reaching the frame bottom pins the bar to the bottom edge", () => {
  const placement = computeSelectionActionsPlacement(
    FRAME, BAR, rect(100, 690), { top: 100, bottom: 690 },
  );
  assert.equal(placement.top, 560);
});

test("a selection entirely above the frame releases the clamp", () => {
  const placement = computeSelectionActionsPlacement(
    FRAME, BAR, rect(-500, -480), { top: -500, bottom: -300 },
  );
  assert.equal(placement.top, -570);
});

test("a selection entirely below the frame releases the clamp", () => {
  const placement = computeSelectionActionsPlacement(
    FRAME, BAR, rect(800, 820), { top: 800, bottom: 900 },
  );
  assert.equal(placement.top, 650);
});

test("a bare caret anchors and bounds itself", () => {
  // The caret path passes one rectangle for both roles; a visible caret is
  // still clamped, so the bar cannot be pushed off the bottom edge.
  const placement = computeSelectionActionsPlacement(FRAME, BAR, rect(300, 330));
  assert.deepEqual(placement, computeSelectionActionsPlacement(
    FRAME, BAR, rect(300, 330), { top: 300, bottom: 330 },
  ));
});
