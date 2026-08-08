/** Load and initialize Continuity only when an editor route needs it. */
export async function loadContinuityEditor(options = {}) {
  const module = await import("./index.js");
  await module.initialize(options.wasm === undefined ? {} : { wasm: options.wasm });
  if (options.registry) module.defineContinuityEditor(options.registry);
  return module;
}
