import type { Ref } from "vue";
import type {
  ContinuityEditorController,
  ContinuityEditorControllerOptions,
} from "./controller.js";
import type { ContinuityEditorElement } from "./index.js";

export function useContinuityEditor(
  element: Ref<ContinuityEditorElement | null | undefined>,
  options: Ref<ContinuityEditorControllerOptions | null | undefined>,
): Readonly<Ref<ContinuityEditorController | undefined>>;
