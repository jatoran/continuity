/** Create an optional backend-neutral, newest-wins async persistence queue. */
export function createCommitQueue(options) {
  if (typeof options?.save !== "function") throw new TypeError("save callback is required");
  const debounceMs = normalizeDelay(options.debounceMs, 250);
  const maxDelayMs = normalizeDelay(options.maxDelayMs, 1_000);
  let debounceTimer;
  let firstQueuedAt = 0;
  let inFlight;
  let latest;
  let lastError;
  let isDisposed = false;

  function enqueue(value) {
    if (isDisposed) throw new Error("Continuity commit queue is disposed");
    latest = value;
    firstQueuedAt ||= performance.now();
    schedule();
  }

  function enqueueChange(detail) {
    if (detail.commitOrigin === "user") enqueue(detail.snapshot);
  }

  function schedule() {
    clearTimeout(debounceTimer);
    const elapsed = performance.now() - firstQueuedAt;
    debounceTimer = setTimeout(run, Math.max(0, Math.min(debounceMs, maxDelayMs - elapsed)));
  }

  async function run() {
    clearTimeout(debounceTimer);
    debounceTimer = undefined;
    if (inFlight || latest === undefined) return;
    const value = latest;
    latest = undefined;
    firstQueuedAt = 0;
    inFlight = Promise.resolve(options.save(value));
    try {
      await inFlight;
      lastError = undefined;
    } catch (error) {
      lastError = error;
      options.onError?.(error, value);
    } finally {
      inFlight = undefined;
      if (latest !== undefined) schedule();
    }
  }

  async function flush() {
    clearTimeout(debounceTimer);
    debounceTimer = undefined;
    while (latest !== undefined || inFlight) {
      await run();
      if (inFlight) await inFlight.catch(() => {});
    }
    if (lastError) throw lastError;
  }

  async function dispose(disposition = {}) {
    if (isDisposed) return;
    if (disposition.flush !== false) await flush();
    clearTimeout(debounceTimer);
    latest = undefined;
    isDisposed = true;
  }

  return {
    enqueue,
    enqueueChange,
    flush,
    dispose,
    get isPending() { return latest !== undefined || Boolean(inFlight); },
  };
}

function normalizeDelay(value, fallback) {
  return Number.isFinite(value) && value >= 0 ? value : fallback;
}
