import { onScopeDispose, shallowRef, unref, watch } from "vue";
import { attachContinuityEditor } from "./src/controller.js";
import "./index.js";

/** Vue 3 composable implementing Continuity's controlled snapshot contract. */
export function useContinuityEditor(elementRef, optionsRef) {
  const controller = shallowRef();
  let attachedElement;
  const stop = watch([elementRef, optionsRef], ([element, options]) => {
    element = unref(element);
    options = unref(options);
    if (!element || !options) return;
    if (element !== attachedElement) {
      controller.value?.dispose();
      controller.value = attachContinuityEditor(element, options);
      attachedElement = element;
      return;
    }
    controller.value.setCallbacks(options.callbacks);
    controller.value.configure(options);
    void controller.value.synchronize(options.value, options.revision)
      .catch((error) => options.callbacks?.onError?.(error));
  }, { immediate: true });
  onScopeDispose(() => {
    stop();
    controller.value?.dispose();
  });
  return controller;
}
