import type { ContinuityEditorControllerOptions } from "./controller.js";
import type { ContinuityEditorElement } from "./index.js";

export type ContinuitySvelteAction = Readonly<{
  update(options: ContinuityEditorControllerOptions): void;
  destroy(): void;
}>;

export function continuityEditor(
  node: ContinuityEditorElement,
  options: ContinuityEditorControllerOptions,
): ContinuitySvelteAction;
