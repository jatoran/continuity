import initializeBindings, { RawEditor } from "../internal/continuity_wasm.js";
import { utf8ByteToUtf16 } from "./coordinates.js";

let initializationPromise;
let isInitialized = false;

/** Error raised before a stale host replacement can overwrite editor state. */
export class RevisionConflictError extends Error {
  constructor(expectedRevision, actualRevision) {
    super(`revision mismatch: expected ${expectedRevision}, actual ${actualRevision}`);
    this.name = "RevisionConflictError";
    this.expectedRevision = expectedRevision;
    this.actualRevision = actualRevision;
  }
}

/** Typed initialization failure with the two common browser-hosting remedies. */
export class ContinuityInitError extends Error {
  constructor(cause) {
    super(
      "Continuity WebAssembly initialization failed. Serve the .wasm asset as " +
      "application/wasm and allow 'wasm-unsafe-eval' in the page CSP.",
      { cause },
    );
    this.name = "ContinuityInitError";
    this.code = "wasm-initialization-failed";
  }
}

/** Initialize the internal WebAssembly module once for the current agent. */
export function initialize(options = {}) {
  if (!initializationPromise) {
    const input = options.wasm === undefined
      ? undefined
      : { module_or_path: options.wasm };
    initializationPromise = initializeBindings(input)
      .then(() => {
        isInitialized = true;
      })
      .catch((error) => {
        initializationPromise = undefined;
        throw error instanceof ContinuityInitError ? error : new ContinuityInitError(error);
      });
  }
  return initializationPromise;
}

/** Synchronous, ephemeral editor owned by the constructing JavaScript agent. */
export class Editor {
  #isAscii;
  #raw;
  #snapshot;

  constructor(initialValue = "", initialRevision = 0) {
    if (!isInitialized) {
      throw new Error("initialize() must resolve before constructing Editor");
    }
    this.#raw = RawEditor.fromSnapshot(
      String(initialValue),
      BigInt(normalizeRevision(initialRevision, "initialRevision")),
    );
    this.#isAscii = !/[^\x00-\x7F]/u.test(initialValue);
    this.#snapshot = {
      text: String(initialValue),
      revision: initialRevision,
      selections: [caretSelection()],
      isReadOnly: false,
    };
  }

  setSelections(selections) {
    this.#requireLive().set_selections_json(JSON.stringify(selections));
    this.#snapshot = { ...this.#snapshot, selections };
  }

  insertText(text, timestampMs = Date.now()) {
    return this.#applyChange(this.#requireLive().insert_text(text, timestampMs));
  }

  deleteBackward(timestampMs = Date.now()) {
    return this.#applyChange(this.#requireLive().delete_backward(timestampMs));
  }

  deleteForward(timestampMs = Date.now()) {
    return this.#applyChange(this.#requireLive().delete_forward(timestampMs));
  }

  indent(timestampMs = Date.now()) {
    return this.#applyChange(this.#requireLive().indent(timestampMs));
  }

  outdent(timestampMs = Date.now()) {
    return this.#applyChange(this.#requireLive().outdent(timestampMs));
  }

  executeCommand(command, timestampMs = Date.now()) {
    const raw = this.#requireLive();
    const change = this.#applyChange(raw.execute_command(String(command), timestampMs));
    if (!change) this.#snapshot = JSON.parse(raw.snapshot_json());
    return change;
  }

  undo(timestampMs = Date.now()) {
    return this.#applyChange(this.#requireLive().undo(timestampMs));
  }

  redo(timestampMs = Date.now()) {
    return this.#applyChange(this.#requireLive().redo(timestampMs));
  }

  redoAlternate(timestampMs = Date.now()) {
    return this.#applyChange(this.#requireLive().redo_alternate(timestampMs));
  }

  replaceValue(value, expectedRevision, timestampMs = Date.now()) {
    expectedRevision = normalizeRevision(expectedRevision, "expectedRevision");
    const actualRevision = this.snapshot().revision;
    if (actualRevision !== expectedRevision) {
      throw new RevisionConflictError(expectedRevision, actualRevision);
    }
    return this.#applyChange(
      this.#requireLive().reconcile_text_if_revision(
        value,
        BigInt(expectedRevision),
        timestampMs,
      ),
    );
  }

  snapshot() {
    this.#requireLive();
    return this.#snapshot;
  }

  projection() {
    return JSON.parse(this.#requireLive().projection_json());
  }

  /** Compact projection for browser presentation without byte-pair maps. */
  presentation() {
    return JSON.parse(this.#requireLive().presentation_json());
  }

  /** Project only a source-line range for an existing presentation. */
  presentationRange(startLine, endLine) {
    return JSON.parse(this.#requireLive().presentation_range_json(startLine, endLine));
  }

  /**
   * Capture the undo history as plain JSON a host can persist across an
   * unmount. `maxGroups` keeps only that many of the newest undo groups; the
   * default of zero keeps all of them.
   */
  exportHistory(options = {}) {
    return JSON.parse(this.#requireLive().history_json(normalizeMaxGroups(options.maxGroups)));
  }

  /**
   * Adopt a history captured by `exportHistory`. The document must hold the
   * content the history was taken from — undo replays recorded inverse edits
   * against the live text, so a mismatch is refused rather than applied.
   */
  importHistory(history) {
    this.#requireLive().restore_history_json(
      typeof history === "string" ? history : JSON.stringify(history),
    );
  }

  linearMemoryBytes() {
    return this.#requireLive().linear_memory_bytes();
  }

  destroy() {
    if (this.#raw) {
      this.#raw.free();
      this.#raw = undefined;
    }
  }

  #requireLive() {
    if (!this.#raw) {
      throw new Error("Editor has been destroyed");
    }
    return this.#raw;
  }

  #applyChange(json) {
    const change = JSON.parse(json);
    if (!change) {
      return null;
    }
    let text = this.#snapshot.text;
    for (const edit of change.edits) {
      const start = this.#isAscii ? edit.at : utf8ByteToUtf16(text, edit.at);
      const end = this.#isAscii
        ? start + edit.removedBytes
        : utf8ByteToUtf16(text, edit.at + edit.removedBytes);
      edit.startUtf16 = start;
      edit.endUtf16 = end;
      edit.startUtf16InLine = start - (text.lastIndexOf("\n", start - 1) + 1);
      edit.endUtf16InLine = end - (text.lastIndexOf("\n", end - 1) + 1);
      text = `${text.slice(0, start)}${edit.insertedText}${text.slice(end)}`;
      this.#isAscii &&= !/[^\x00-\x7F]/u.test(edit.insertedText);
    }
    this.#snapshot = {
      text,
      revision: change.revisionAfter,
      selections: change.selectionsAfter,
      isReadOnly: this.#snapshot.isReadOnly,
    };
    return change;
  }
}

/** Apply one named component operation to the editor and return its change. */
export function applyEditorOperation(editor, kind, value, timestamp) {
  const operations = {
    insert: () => editor.insertText(value, timestamp),
    deleteBackward: () => editor.deleteBackward(timestamp),
    deleteForward: () => editor.deleteForward(timestamp),
    indent: () => editor.indent(timestamp),
    outdent: () => editor.outdent(timestamp),
    undo: () => editor.undo(timestamp),
    redo: () => editor.redo(timestamp),
    command: () => editor.executeCommand(value, timestamp),
  };
  return operations[kind]?.();
}

function caretSelection() {
  const position = { line: 0, byteInLine: 0 };
  return { anchor: position, head: position, kind: "caret" };
}

function normalizeMaxGroups(value) {
  if (value === undefined || value === null) return 0;
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new RangeError("maxGroups must be a non-negative safe integer");
  }
  return value;
}

function normalizeRevision(value, name) {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new RangeError(`${name} must be a non-negative safe integer`);
  }
  return value;
}
