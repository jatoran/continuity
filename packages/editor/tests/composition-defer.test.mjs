import assert from "node:assert/strict";
import test from "node:test";

const moduleUrl = new URL(
  "./node_modules/@continuity-editor/editor/src/composition_defer.js",
  import.meta.url,
);
const { planDeferredHostReplace } = await import(moduleUrl);

test("nothing deferred yields no replacement", () => {
  assert.equal(planDeferredHostReplace(null, { text: "a", revision: 3 }), null);
});

test("a deferred replacement matching the committed text is dropped", () => {
  const plan = planDeferredHostReplace({ value: "hi", revision: 4 }, { text: "hi", revision: 6 });
  assert.equal(plan, null);
});

test("a stale echo whose revision no longer matches is dropped", () => {
  // The controlled adapter echoes a pre-composition revision; after the user's
  // word commits, the engine has advanced, so applying it would clobber typing.
  const plan = planDeferredHostReplace({ value: "old", revision: 4 }, { text: "old swe", revision: 5 });
  assert.equal(plan, null);
});

test("a replacement at the current revision applies", () => {
  const plan = planDeferredHostReplace({ value: "fresh", revision: 5 }, { text: "stale", revision: 5 });
  assert.deepEqual(plan, { value: "fresh", revision: 5 });
});

test("a forced replacement re-anchors to the current revision", () => {
  const plan = planDeferredHostReplace({ value: "forced", revision: null }, { text: "prev", revision: 8 });
  assert.deepEqual(plan, { value: "forced", revision: 8 });
});
