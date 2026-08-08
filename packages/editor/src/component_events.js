/** Dispatch one composed component protocol event across the shadow boundary. */
export function dispatchEditorEvent(host, name, detail) {
  host.dispatchEvent(new CustomEvent(name, {
    bubbles: true,
    composed: true,
    detail,
  }));
}

/** Publish a browser-policy shortcut decision without consuming the keydown. */
export function dispatchShortcutSuppressed(host, detail) {
  dispatchEditorEvent(host, "continuity-shortcut-suppressed", { version: 1, ...detail });
}

/**
 * Publish the source-line window the reader can currently see.
 *
 * The event names the edges `firstLine` / `lastLine` while the getter returns
 * `startLine` / `endLine`; both describe the same inclusive source-line window,
 * and the translation lives here so there is one place that knows it.
 */
export function dispatchEditorViewport(host, range) {
  dispatchEditorEvent(host, "continuity-viewport", {
    version: 1,
    firstLine: range.startLine,
    lastLine: range.endLine,
  });
}

/** Publish the browser-observable paint-ready timing boundary. */
export function dispatchEditorFrame(host, revision, inputStartedAt, paintedAt) {
  dispatchEditorEvent(host, "continuity-frame", {
    version: 1,
    revision,
    inputStartedAt,
    paintedAt,
    latencyMs: paintedAt - inputStartedAt,
  });
}

/**
 * Bind the host-facing emitters that carry a monotonic sequence. The counter
 * lives here rather than on the element because a request and the change it
 * causes must be orderable against each other by a host that only sees events.
 */
export function createEditorEmitters(host) {
  let sequence = 0;
  return {
    emitRequest: (kind, payload) => dispatchEditorEvent(host, "continuity-request", {
      version: 1, sequence: ++sequence, kind, ...payload,
    }),
    emitChange: (detail) => dispatchEditorEvent(host, "continuity-change", {
      version: 1, sequence: ++sequence, ...detail,
    }),
    emitError: (error) => dispatchEditorEvent(host, "continuity-error", { version: 1, error }),
  };
}

/** Return the live engine or raise the component lifecycle contract error. */
export function requireLiveEditor(editor, isDestroyed) {
  if (isDestroyed) {
    throw new Error("Continuity editor has been destroyed");
  }
  if (!editor) {
    throw new Error("Continuity editor is not ready; await element.ready");
  }
  return editor;
}
