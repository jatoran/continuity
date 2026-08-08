import {
  createElement,
  forwardRef,
  useCallback,
  useLayoutEffect,
  useRef,
  useState,
} from "react";

import { RevisionConflictError } from "./src/engine.js";
import {
  applyEditorConfiguration,
  initializeControlledElement,
  synchronizeControlledSnapshot,
} from "./src/controlled.js";
import "./index.js";

const EVENT_NAMES = [
  "continuity-change",
  "continuity-request",
  "continuity-frame",
  "continuity-ready",
  "continuity-destroy",
  "continuity-error",
  "continuity-shortcut-suppressed",
];

/** React adapter for the controlled, revision-checked Web Component contract. */
export const ContinuityEditor = forwardRef(function ContinuityEditor(props, forwardedRef) {
  const {
    value,
    revision,
    readOnly = false,
    spellcheck = true,
    shortcutPolicy = "browser-safe",
    shortcutBindings,
    railActions,
    theme,
    tabBehavior,
    syntax,
    indentGuides,
    onChange,
    onHostReplacement,
    onRequest,
    onFrame,
    onReady,
    onDestroy,
    onError,
    onShortcutSuppressed,
    onRevisionConflict,
    ...elementProps
  } = props;
  const callbacksRef = useRef();
  callbacksRef.current = {
    onChange,
    onHostReplacement,
    onRequest,
    onFrame,
    onReady,
    onDestroy,
    onError,
    onShortcutSuppressed,
    onRevisionConflict,
  };
  const renderPropsRef = useRef();
  renderPropsRef.current = { value, revision, readOnly, spellcheck, shortcutPolicy, shortcutBindings, railActions, theme, tabBehavior, syntax, indentGuides };
  const initializedElementsRef = useRef(new WeakSet());
  const lastHostSnapshotRef = useRef();
  const elementRef = useRef(null);
  const [element, setElement] = useState(null);
  const listenersRef = useRef(createEventListeners(callbacksRef));

  const attachElement = useCallback((nextElement) => {
    const previousElement = elementRef.current;
    if (previousElement && previousElement !== nextElement) {
      removeEventListeners(previousElement, listenersRef.current);
    }
    elementRef.current = nextElement;
    assignRef(forwardedRef, nextElement);
    if (nextElement) {
      const current = renderPropsRef.current;
      addEventListeners(nextElement, listenersRef.current);
      if (!initializedElementsRef.current.has(nextElement)) {
        initializeControlledElement(nextElement, current.value, current.revision);
        initializedElementsRef.current.add(nextElement);
        lastHostSnapshotRef.current = snapshotIdentity(current.value, current.revision);
      }
      applyEditorConfiguration(nextElement, current);
    }
    setElement(nextElement);
  }, [forwardedRef]);

  useLayoutEffect(() => {
    if (element) {
      applyEditorConfiguration(element, {
        readOnly,
        spellcheck,
        shortcutPolicy,
        shortcutBindings,
        railActions,
        theme,
        tabBehavior,
        syntax,
        indentGuides,
      });
    }
  }, [element, readOnly, spellcheck, shortcutPolicy, shortcutBindings, railActions, theme, tabBehavior, syntax, indentGuides]);

  useLayoutEffect(() => {
    if (!element) {
      return undefined;
    }
    const nextSnapshot = snapshotIdentity(value, revision);
    if (isSameSnapshot(lastHostSnapshotRef.current, nextSnapshot)) {
      return undefined;
    }
    lastHostSnapshotRef.current = nextSnapshot;
    let isCurrent = true;
    void synchronizeControlledSnapshot(element, nextSnapshot.value, nextSnapshot.revision)
      .catch((error) => {
        if (!isCurrent) {
          return;
        }
        if (error instanceof RevisionConflictError && callbacksRef.current.onRevisionConflict) {
          callbacksRef.current.onRevisionConflict(error);
        } else {
          callbacksRef.current.onError?.(error);
        }
      });
    return () => {
      isCurrent = false;
    };
  }, [element, value, revision]);

  return createElement("continuity-editor", { ...elementProps, ref: attachElement });
});

function createEventListeners(callbacksRef) {
  return {
    "continuity-change": (event) => {
      callbacksRef.current.onChange?.(event.detail, event);
      if (event.detail.commitOrigin === "host") {
        callbacksRef.current.onHostReplacement?.(event.detail, event);
      }
    },
    "continuity-request": (event) => callbacksRef.current.onRequest?.(event.detail, event),
    "continuity-frame": (event) => callbacksRef.current.onFrame?.(event.detail, event),
    "continuity-ready": (event) => callbacksRef.current.onReady?.(event.detail, event),
    "continuity-destroy": (event) => callbacksRef.current.onDestroy?.(event),
    "continuity-error": (event) => callbacksRef.current.onError?.(event.detail.error, event),
    "continuity-shortcut-suppressed": (event) => callbacksRef.current.onShortcutSuppressed?.(event.detail, event),
  };
}

function addEventListeners(element, listeners) {
  EVENT_NAMES.forEach((name) => element.addEventListener(name, listeners[name]));
}

function removeEventListeners(element, listeners) {
  EVENT_NAMES.forEach((name) => element.removeEventListener(name, listeners[name]));
}

function assignRef(ref, value) {
  if (typeof ref === "function") {
    ref(value);
  } else if (ref) {
    ref.current = value;
  }
}

function snapshotIdentity(value, revision) {
  return { value: String(value), revision };
}

function isSameSnapshot(left, right) {
  return left?.value === right.value && left?.revision === right.revision;
}
