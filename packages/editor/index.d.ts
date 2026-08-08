export type Position = Readonly<{
  line: number;
  byteInLine: number;
}>;

export type SelectionKind = "caret" | "lineWise" | "blockWise";

export type Selection = Readonly<{
  anchor: Position;
  head: Position;
  kind: SelectionKind;
}>;

export type Range = Readonly<{ start: Position; end: Position }>;
export type ScrollState = Readonly<{ top: number; left: number }>;

/**
 * Inclusive source-line window, in the same coordinate space as
 * `Position.line`. Both edges count partially visible lines, and a wrapped
 * line reports its own source line rather than a visual row.
 */
export type VisibleLineRange = Readonly<{ startLine: number; endLine: number }>;

export type Delta = Readonly<{
  at: number;
  removedBytes: number;
  insertedBytes: number;
}>;

export type AppliedEdit = Readonly<{
  at: number;
  removedBytes: number;
  insertedText: string;
  startLine: number;
  endLine: number;
  /** UTF-16 offsets populated by the JavaScript facade for DOM mirroring. */
  startUtf16: number;
  endUtf16: number;
}>;

export type Change = Readonly<{
  kind: "rawEdit" | "groupedEdit" | "selectionEdit" | "undo" | "redo";
  command: string;
  revisionBefore: number;
  revisionAfter: number;
  selectionsBefore: readonly Selection[];
  selectionsAfter: readonly Selection[];
  deltas: readonly Delta[];
  edits: readonly AppliedEdit[];
  checksumAfter: string;
}>;

export type Snapshot = Readonly<{
  text: string;
  revision: number;
  selections: readonly Selection[];
  isReadOnly: boolean;
}>;

export type Segment = Readonly<{
  kind: "visible" | "hidden" | "replace";
  startByte: number;
  endByte: number;
  replacement: string;
}>;

export type Span = Readonly<{
  kind: string;
  startByte: number;
  endByte: number;
}>;

export type DisplayLine = Readonly<{
  text: string;
  sourceToDisplay: readonly (readonly [number, number])[];
  displayToSource: readonly (readonly [number, number])[];
  segments: readonly Segment[];
}>;

export type Projection = Readonly<{
  blocks: readonly Span[];
  inlines: readonly Span[];
  lines: readonly DisplayLine[];
}>;

export type PresentationLine = Readonly<{
  text: string;
  sourceToDisplay: readonly [];
  displayToSource: readonly [];
  segments: readonly Segment[];
}>;

export type Presentation = Readonly<{
  blocks: readonly Span[];
  inlines: readonly Span[];
  lines: readonly PresentationLine[];
}>;

export type PresentationRange = Readonly<{
  startLine: number;
  endLine: number;
  lineCount: number;
  lineStarts: readonly number[];
  blocks: readonly Span[];
  inlines: readonly Span[];
  lines: readonly PresentationLine[];
}>;

/** One serialized atomic edit inside an exported undo history. */
export type HistoryEditOp =
  | Readonly<{ kind: "insert"; at: Position; text: string }>
  | Readonly<{ kind: "delete"; range: Range }>
  | Readonly<{ kind: "replace"; range: Range; text: string }>;

export type HistoryEditRecord = Readonly<{
  op: HistoryEditOp;
  inverseOp: HistoryEditOp;
  revisionBefore: number;
  revisionAfter: number;
  selectionsBefore: readonly Selection[];
  selectionsAfter: readonly Selection[];
}>;

export type HistoryGroup = Readonly<{
  /** Index into `HistoryState.groups`; null for a root branch. */
  parent: number | null;
  timestampMs: number;
  command: string;
  ops: readonly HistoryEditRecord[];
}>;

/**
 * A portable undo history. Treat it as opaque JSON: persist it and hand it
 * back to `importHistory`. The shape is stable within a `version`.
 */
export type HistoryState = Readonly<{
  version: 1;
  /** Revision the history was captured at, for host bookkeeping. */
  revision: number;
  /** Content checksum; importing into different text is refused. */
  checksum: string;
  /** Index of the group whose edits are currently applied. */
  currentGroupIndex: number | null;
  /** Whether older groups were dropped to satisfy `maxGroups`. */
  isTruncated: boolean;
  groups: readonly HistoryGroup[];
}>;

export type HistoryExportOptions = Readonly<{
  /** Keep only this many of the newest undo groups; 0 (default) keeps all. */
  maxGroups?: number;
}>;

export type WasmInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export function initialize(options?: Readonly<{ wasm?: WasmInput }>): Promise<void>;

export class ContinuityInitError extends Error {
  readonly code: "wasm-initialization-failed";
  readonly cause: unknown;
}

export class RevisionConflictError extends Error {
  readonly expectedRevision: number;
  readonly actualRevision: number;
}

export class Editor {
  constructor(initialValue?: string, initialRevision?: number);
  setSelections(selections: readonly Selection[]): void;
  insertText(text: string, timestampMs?: number): Change | null;
  deleteBackward(timestampMs?: number): Change | null;
  deleteForward(timestampMs?: number): Change | null;
  indent(timestampMs?: number): Change | null;
  outdent(timestampMs?: number): Change | null;
  executeCommand(command: string, timestampMs?: number): Change | null;
  undo(timestampMs?: number): Change | null;
  redo(timestampMs?: number): Change | null;
  redoAlternate(timestampMs?: number): Change | null;
  replaceValue(value: string, expectedRevision: number, timestampMs?: number): Change | null;
  snapshot(): Snapshot;
  projection(): Projection;
  presentation(): Presentation;
  presentationRange(startLine: number, endLine: number): PresentationRange;
  exportHistory(options?: HistoryExportOptions): HistoryState;
  /** Throws when the history was captured from different content. */
  importHistory(history: HistoryState | string): void;
  linearMemoryBytes(): number;
  destroy(): void;
}

export type ContinuityChangeDetail = Readonly<{
  version: 1;
  sequence: number;
  source: "insert" | "deleteBackward" | "deleteForward" | "indent" | "outdent" | "undo" | "redo" |
    "compositionCommit" | "browserReconcile" | "hostReplacement" | `command:${string}`;
  /** Stable persistence boundary: hosts persist only user-origin commits. */
  commitOrigin: "user" | "host";
  change: Change;
  snapshot: Snapshot;
}>;

export type ContinuityRequestDetail = Readonly<{
  version: 1;
  sequence: number;
}> & (
  | Readonly<{ kind: "openLink"; label: string; href: string }>
  | Readonly<{ kind: "contextMenu"; clientX: number; clientY: number; isLongPressSelection: boolean }>
  | Readonly<{ kind: "copyText"; text: string }>
  /** The browser refused clipboard-read; answer with `insertText(text)`. */
  | Readonly<{ kind: "pasteText" }>
  | Readonly<{ kind: "filesDropped"; files: readonly File[] }>
  /** A registered rail action that names neither a command nor a callback. */
  | Readonly<{ kind: "railAction"; actionId: string }>
);

export type ContinuityFrameDetail = Readonly<{
  version: 1;
  revision: number;
  inputStartedAt: number;
  /** Fast projection completion in the animation frame submitted for paint. */
  paintedAt: number;
  latencyMs: number;
}>;

export type ContinuityReadyDetail = Readonly<{
  version: 1;
  snapshot: Snapshot;
}>;

/**
 * The source-line window the reader can see, published when it changes.
 *
 * Emitted once after `continuity-ready`, then at most once per animation
 * frame, and only when the window actually differs from the one before it.
 * `firstLine` / `lastLine` are the same inclusive edges `visibleLineRange()`
 * returns as `startLine` / `endLine`.
 */
export type ContinuityViewportDetail = Readonly<{
  version: 1;
  firstLine: number;
  lastLine: number;
}>;

export type ContinuityErrorDetail = Readonly<{
  version: 1;
  error: unknown;
}>;

export type ContinuityShortcutSuppressedDetail = Readonly<{
  version: 1;
  chord: string;
  command: string;
  policy: ShortcutPolicy;
}>;

export type ShortcutPolicy = "browser-safe" | "editor-first" | "none";
export type ShortcutBindings = Readonly<Record<string, string | null>> |
  ReadonlyMap<string, string | null>;
export type ShortcutBinding = Readonly<{
  chord: string;
  command: string;
  isBrowserSafe: boolean;
}>;

export function listShortcutBindings(): readonly ShortcutBinding[];

/**
 * A host-supplied command-rail button. The id is namespaced (`vendor:action`)
 * so it can never collide with a built-in, and it is the key the arrangement
 * is persisted under: keep it stable across releases.
 *
 * Exactly one behaviour applies, in this order: `command` runs a
 * storage-neutral engine command, `run` calls back into the host, and a
 * descriptor with neither raises `continuity-request` with `kind: "railAction"`
 * so a declarative host can answer from an event listener.
 */
export type RailAction = Readonly<{
  id: string;
  label: string;
  /** Text mark for the button; defaults to the label's first character. */
  glyph?: string;
  /** Builds the button's icon node. Host-built elements only, never markup. */
  icon?: () => Element;
  command?: string;
  run?: (editor: ContinuityEditorElement) => void;
  /** Re-evaluated on every commit and selection change. */
  isEnabled?: (snapshot: Snapshot) => boolean;
  /** Defaults to true; false starts the action off until a user enables it. */
  isEnabledByDefault?: boolean;
}>;

export type BuiltInRailAction = Readonly<{
  id: string;
  label: string;
  /** Null for actions with no storage-neutral command (caret motion). */
  command: string | null;
  isEnabledByDefault: boolean;
}>;

export function listBuiltInRailActions(): readonly BuiltInRailAction[];

export class ContinuityEditorElement extends HTMLElement {
  readonly ready: Promise<this>;
  value: string;
  initialRevision: number;
  readOnly: boolean;
  spellcheck: boolean;
  shortcutPolicy: ShortcutPolicy;
  shortcutBindings: ShortcutBindings;
  syntax: "markdown" | "plain";
  /** Reflected by the `indent-guides` attribute; defaults to off. */
  indentGuides: "on" | "off";
  /** Whether a browser IME composition is active; host writes defer until it ends. */
  readonly composing: boolean;
  snapshot(): Snapshot;
  /** Returns null when deferred because an IME composition is active. */
  replaceValue(value: string, expectedRevision: number, timestampMs?: number): Change | null;
  /**
   * Insert text at the current selection, replacing it. Use this to answer a
   * `continuity-request` of kind `pasteText`, which the editor emits when the
   * browser refuses it clipboard-read access.
   */
  insertText(text: string): Change | null;
  executeCommand(command: string, timestampMs?: number): Change | null;
  /** Add one host action to the command rail; call the result to remove it. */
  registerRailAction(action: RailAction): () => void;
  /** Host actions on the rail. Assigning replaces the whole host set. */
  railActions: readonly RailAction[];
  setSelections(selections: readonly Selection[], options?: Readonly<{ reveal?: boolean }>): Snapshot;
  revealRange(range: Range, options?: Readonly<{ align?: "nearest" | "center" }>): void;
  /**
   * Paint a named set of source ranges without touching selection or history.
   * The id must be a CSS identifier: it names the theming property
   * `--continuity-decoration-<id>` and the shadow part `decoration-<id>`.
   * Ranges are positions, not anchors — re-set them after a change. Returns the
   * number of ranges retained; an empty list removes the set.
   */
  setDecorations(id: string, ranges: readonly Range[]): number;
  /** Remove one decoration set, or every set when no id is given. */
  clearDecorations(id?: string): void;
  /** Capture undo history for restoration after an unmount. */
  exportHistory(options?: HistoryExportOptions): HistoryState;
  /** Adopt a captured history; throws unless the content matches. */
  importHistory(history: HistoryState | string): void;
  getScrollState(): ScrollState;
  restoreScrollState(state: ScrollState): void;
  /**
   * The source lines currently on screen, or `null` when there is nothing to
   * measure: before the first layout, or while the host keeps the editor in a
   * `display: none` tab. This is the visible window, not the larger one the
   * projection realizes ahead of the reader.
   */
  visibleLineRange(): VisibleLineRange | null;
  linearMemoryBytes(): number;
  focus(options?: FocusOptions): void;
  destroy(): void;
}

export class ContinuityRendererElement extends HTMLElement {
  readonly ready: Promise<this>;
  value: string;
  syntax: "markdown" | "plain";
  getScrollState(): ScrollState;
  restoreScrollState(state: ScrollState): void;
  linearMemoryBytes(): number;
  destroy(): void;
}

export function defineContinuityEditor(registry?: CustomElementRegistry): void;

declare global {
  interface HTMLElementTagNameMap {
    "continuity-editor": ContinuityEditorElement;
    "continuity-renderer": ContinuityRendererElement;
  }

  interface GlobalEventHandlersEventMap {
    "continuity-change": CustomEvent<ContinuityChangeDetail>;
    "continuity-request": CustomEvent<ContinuityRequestDetail>;
    "continuity-frame": CustomEvent<ContinuityFrameDetail>;
    "continuity-ready": CustomEvent<ContinuityReadyDetail>;
    "continuity-viewport": CustomEvent<ContinuityViewportDetail>;
    "continuity-destroy": CustomEvent<Readonly<{ version: 1 }>>;
    "continuity-error": CustomEvent<ContinuityErrorDetail>;
    "continuity-shortcut-suppressed": CustomEvent<ContinuityShortcutSuppressedDetail>;
  }
}
