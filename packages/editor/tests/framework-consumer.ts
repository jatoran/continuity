import { ref } from "vue";

import type {
  ContinuityEditorElement,
  ContinuityViewportDetail,
  VisibleLineRange,
} from "@continuity-editor/editor";
import { continuityEditor } from "@continuity-editor/editor/svelte";
import { useContinuityEditor } from "@continuity-editor/editor/vue";

declare const element: ContinuityEditorElement;

// The visible-window primitive a host builds scroll-linked chrome on: `null`
// is part of the getter's type, and the event's edges are plain line numbers.
const visible: VisibleLineRange | null = element.visibleLineRange();
const topLine: number = visible ? visible.startLine : 0;
element.addEventListener("continuity-viewport", (event) => {
  const detail: ContinuityViewportDetail = event.detail;
  const trailAnchor: number = detail.firstLine + detail.lastLine + topLine;
  void trailAnchor;
});

const options = { value: "# Framework contract", revision: 0 } as const;
const action = continuityEditor(element, options);
action.update({ ...options, revision: 1 });
action.destroy();

const elementRef = ref<ContinuityEditorElement>();
const optionsRef = ref(options);
useContinuityEditor(elementRef, optionsRef);
