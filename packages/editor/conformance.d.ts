import type { ContinuityEditorElement } from "./index.js";

export function runContinuityConformance(options?: Readonly<{
  createElement?: () => ContinuityEditorElement;
}>): Promise<Readonly<{ passed: true; assertions: number; changes: number }>>;
