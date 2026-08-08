import assert from "node:assert/strict";
import test from "node:test";

const packageRoot = new URL("./node_modules/@continuity-editor/editor/", import.meta.url);

test("top-level, controller, lazy, and helper imports are SSR safe", async () => {
  const modules = await Promise.all([
    import(new URL("index.js", packageRoot)),
    import(new URL("controller.js", packageRoot)),
    import(new URL("lazy.js", packageRoot)),
    import(new URL("commit-queue.js", packageRoot)),
  ]);
  assert.equal(typeof modules[0].ContinuityEditorElement, "function");
  assert.equal(typeof modules[1].attachContinuityEditor, "function");
  assert.equal(typeof modules[2].loadContinuityEditor, "function");
  assert.equal(typeof modules[3].createCommitQueue, "function");
});

test("commit queue is single-in-flight, newest-wins, and filters host echoes", async () => {
  const saved = [];
  let release;
  const { createCommitQueue } = await import(new URL("commit-queue.js", packageRoot));
  const queue = createCommitQueue({
    debounceMs: 0,
    maxDelayMs: 0,
    save(value) {
      saved.push(value.text);
      if (saved.length === 1) return new Promise((resolve) => { release = resolve; });
    },
  });
  queue.enqueue({ text: "one" });
  await new Promise((resolve) => setTimeout(resolve, 0));
  queue.enqueue({ text: "two" });
  queue.enqueue({ text: "three" });
  queue.enqueueChange({ commitOrigin: "host", snapshot: { text: "host" } });
  release();
  await queue.flush();
  assert.deepEqual(saved, ["one", "three"]);
  await queue.dispose();
});

test("initialization failures are typed and include hosting guidance", async () => {
  const { ContinuityInitError, initialize } = await import(new URL("index.js", packageRoot));
  await assert.rejects(
    initialize({ wasm: new Uint8Array([0, 1, 2, 3]) }),
    (error) => error instanceof ContinuityInitError
      && error.code === "wasm-initialization-failed"
      && error.message.includes("application/wasm")
      && error.message.includes("wasm-unsafe-eval"),
  );
});
