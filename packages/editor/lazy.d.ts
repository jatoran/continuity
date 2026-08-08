import type * as Continuity from "./index.js";

export function loadContinuityEditor(options?: Readonly<{
  wasm?: Continuity.WasmInput;
  registry?: CustomElementRegistry;
}>): Promise<typeof Continuity>;
