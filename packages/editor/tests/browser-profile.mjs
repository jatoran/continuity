const SHOULD_PROFILE = process.env.CONTINUITY_BROWSER_CPU_PROFILE === "1";

/** Start optional Chromium CPU sampling for local browser-check diagnosis. */
export async function startBrowserProfile(page) {
  if (!SHOULD_PROFILE) return;
  await page.send("Profiler.enable");
  await page.send("Profiler.setSamplingInterval", { interval: 100 });
  await page.send("Profiler.start");
}

/** Stop optional CPU sampling and return a compact hottest-frame report. */
export async function stopBrowserProfile(page) {
  if (!SHOULD_PROFILE) return null;
  const { profile } = await page.send("Profiler.stop");
  const nodes = new Map(profile.nodes.map((node) => [node.id, node]));
  const totals = new Map();
  profile.samples?.forEach((nodeId, index) => {
    const frame = nodes.get(nodeId)?.callFrame;
    if (!frame) return;
    const key = `${frame.functionName || "(anonymous)"} @ ${frame.url}:${frame.lineNumber + 1}`;
    totals.set(key, (totals.get(key) ?? 0) + (profile.timeDeltas?.[index] ?? 0));
  });
  return [...totals.entries()]
    .sort((left, right) => right[1] - left[1])
    .slice(0, 30)
    .map(([frame, microseconds]) => ({ frame, milliseconds: microseconds / 1_000 }));
}
