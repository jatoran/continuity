import {
  applyEditorConfiguration,
  initializeControlledElement,
  synchronizeControlledSnapshot,
} from "./controlled.js";

const EVENT_CALLBACKS = new Map([
  ["continuity-change", "onChange"],
  ["continuity-request", "onRequest"],
  ["continuity-frame", "onFrame"],
  ["continuity-ready", "onReady"],
  ["continuity-destroy", "onDestroy"],
  ["continuity-error", "onError"],
  ["continuity-shortcut-suppressed", "onShortcutSuppressed"],
]);

/** Attach reusable controlled-state semantics to a Continuity Web Component. */
export function attachContinuityEditor(element, options) {
  return new ContinuityEditorController(element, options);
}

/** Framework-neutral owner of one controlled Continuity element attachment. */
export class ContinuityEditorController {
  #callbacks;
  #configuration;
  #isDisposed = false;
  #lastHostSnapshot;
  #listeners;

  constructor(element, options) {
    if (!element || typeof element.addEventListener !== "function") {
      throw new TypeError("attachContinuityEditor requires a Continuity editor element");
    }
    if (!options || !Number.isSafeInteger(options.revision) || options.revision < 0) {
      throw new RangeError("controlled revision must be a non-negative safe integer");
    }
    if (!("value" in options)) {
      throw new TypeError("controlled value is required");
    }
    this.element = element;
    this.#callbacks = { ...(options.callbacks ?? {}) };
    this.#configuration = { ...options };
    this.#listeners = createEventListeners(() => this.#callbacks);
    addEventListeners(element, this.#listeners);
    initializeControlledElement(element, options.value, options.revision);
    this.#lastHostSnapshot = snapshotIdentity(options.value, options.revision);
    applyEditorConfiguration(element, options);
  }

  /** Replace host callbacks without reattaching DOM listeners. */
  setCallbacks(callbacks = {}) {
    this.#requireAttached();
    this.#callbacks = { ...callbacks };
  }

  /** Apply host-owned read-only, shortcut, theme, and Tab configuration. */
  configure(configuration = {}) {
    this.#requireAttached();
    this.#configuration = { ...this.#configuration, ...configuration };
    applyEditorConfiguration(this.element, this.#configuration);
  }

  /** Reconcile a host snapshot, rejecting rather than overwriting newer typing. */
  async synchronize(value, revision) {
    this.#requireAttached();
    const nextSnapshot = snapshotIdentity(value, revision);
    if (isSameSnapshot(this.#lastHostSnapshot, nextSnapshot)) {
      return null;
    }
    this.#lastHostSnapshot = nextSnapshot;
    return synchronizeControlledSnapshot(this.element, nextSnapshot.value, nextSnapshot.revision);
  }

  /** Intentionally replace the current engine snapshot without a stale host revision. */
  async replaceCurrent(value) {
    this.#requireAttached();
    await this.element.ready;
    const revision = this.element.snapshot().revision;
    this.#lastHostSnapshot = snapshotIdentity(value, revision);
    return this.element.replaceValue(String(value), revision);
  }

  /** Set engine selections and optionally reveal the primary selection. */
  setSelections(selections, options) {
    this.#requireAttached();
    this.element.setSelections(selections, options);
  }

  /** Reveal a source range without changing the document. */
  revealRange(range, options) {
    this.#requireAttached();
    this.element.revealRange(range, options);
  }

  /** Paint one named set of source ranges; an empty list removes the set. */
  setDecorations(id, ranges) {
    this.#requireAttached();
    return this.element.setDecorations(id, ranges);
  }

  /** Remove one decoration set, or every set when no id is given. */
  clearDecorations(id) {
    this.#requireAttached();
    this.element.clearDecorations(id);
  }

  /** Capture the undo history for restoration across an unmount. */
  exportHistory(options) {
    this.#requireAttached();
    return this.element.exportHistory(options);
  }

  /** Adopt a captured history for this same document content. */
  importHistory(history) {
    this.#requireAttached();
    this.element.importHistory(history);
  }

  /** Capture the editor viewport for per-document restoration. */
  getScrollState() {
    this.#requireAttached();
    return this.element.getScrollState();
  }

  /** Restore a previously captured editor viewport. */
  restoreScrollState(state) {
    this.#requireAttached();
    this.element.restoreScrollState(state);
  }

  /** Report the shared WASM linear-memory allocation in bytes. */
  linearMemoryBytes() {
    this.#requireAttached();
    return this.element.linearMemoryBytes();
  }

  /** Detach controller listeners without destroying the host-owned element. */
  dispose() {
    if (this.#isDisposed) {
      return;
    }
    removeEventListeners(this.element, this.#listeners);
    this.#callbacks = {};
    this.#isDisposed = true;
  }

  #requireAttached() {
    if (this.#isDisposed) {
      throw new Error("ContinuityEditorController is disposed");
    }
  }
}

function createEventListeners(getCallbacks) {
  const listeners = new Map();
  for (const [eventName, callbackName] of EVENT_CALLBACKS) {
    listeners.set(eventName, (event) => {
      const callback = getCallbacks()[callbackName];
      if (callback && eventName === "continuity-destroy") {
        callback(event);
      } else if (callback && eventName === "continuity-error") {
        callback(event.detail.error, event);
      } else if (callback) {
        callback(event.detail, event);
      }
      if (eventName === "continuity-change" && event.detail.commitOrigin === "host") {
        getCallbacks().onHostReplacement?.(event.detail, event);
      }
    });
  }
  return listeners;
}

function addEventListeners(element, listeners) {
  for (const [name, listener] of listeners) {
    element.addEventListener(name, listener);
  }
}

function removeEventListeners(element, listeners) {
  for (const [name, listener] of listeners) {
    element.removeEventListener(name, listener);
  }
}

function snapshotIdentity(value, revision) {
  if (!Number.isSafeInteger(revision) || revision < 0) {
    throw new RangeError("controlled revision must be a non-negative safe integer");
  }
  return { value: String(value), revision };
}

function isSameSnapshot(left, right) {
  return left?.value === right.value && left?.revision === right.revision;
}
