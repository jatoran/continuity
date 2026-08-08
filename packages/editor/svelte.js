import { attachContinuityEditor } from "./src/controller.js";
import "./index.js";

/** Svelte action implementing Continuity's controlled snapshot contract. */
export function continuityEditor(node, options) {
  let controller = attachContinuityEditor(node, options);
  return {
    update(next) {
      controller.setCallbacks(next.callbacks);
      controller.configure(next);
      void controller.synchronize(next.value, next.revision)
        .catch((error) => next.callbacks?.onError?.(error));
    },
    destroy() {
      controller.dispose();
      controller = undefined;
    },
  };
}
