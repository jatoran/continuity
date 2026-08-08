import { ContinuityEditorElement } from "./component.js";
import { ContinuityRendererElement } from "./static_renderer.js";

/** Define `<continuity-editor>` once in the current custom-element registry. */
export function defineContinuityEditor(registry = globalThis.customElements) {
  if (registry && !registry.get("continuity-editor")) {
    registry.define("continuity-editor", ContinuityEditorElement);
  }
  if (registry && !registry.get("continuity-renderer")) {
    registry.define("continuity-renderer", ContinuityRendererElement);
  }
}
