import { Editor, applyEditorOperation, initialize } from "./engine.js";
import { attachCommandRail } from "./command_rail.js";
import { applyEditorAttributes, buildEditorDom } from "./component_dom.js";
import {
  createEditorEmitters, dispatchEditorEvent, dispatchEditorFrame, dispatchEditorViewport,
  dispatchShortcutSuppressed, requireLiveEditor,
} from "./component_events.js";
import { createViewportWatch, handleEditorScroll } from "./component_viewport.js";
import { normalizeNewlines } from "./input_events.js";
import { createRailRegistry } from "./command_rail_registry.js";
import { createTransferHandlers } from "./component_transfer.js";
import { clearProjectionCompositionLine } from "./projection.js";
import {
  copyCodeBlock, copyText, createClipboardHooks, readClipboardText,
} from "./clipboard_bridge.js";
import { selectionsWithInputPrimary } from "./multi_selection.js";
import { handleEditorKeyDown, keyboardHelpText } from "./keyboard.js";
import { EditorInputSynchronizer, inputSelectionKey } from "./input_sync.js";
import { handleBeforeInput } from "./native_input.js";
import { ProjectionPointerGesture } from "./pointer_gesture.js";
import {
  applyVerticalCaret, clearSecondarySelections,
  completeProjectedPointerClick, handlePointerCancel, handlePointerDown, handlePointerMove,
  handlePointerUp, handleSelectStart, handleTouchMove, moveVerticalCaret,
  isSurfaceEvent, routeSurfacePointer,
} from "./component_pointer.js";
import { NativeSelectionWatcher, applyNativeSelection } from "./native_selection.js";
import { TouchSelectionGesture } from "./touch_selection.js";
import { DragAutoScroller } from "./drag_autoscroll.js";
import { createEditorOverlays } from "./component_overlays.js";
import { RenderScheduler } from "./render_scheduler.js";
import { renderEditorPresentation, renderFastPresentation } from "./presentation_render.js";
import { reconcileTextareaMutation } from "./textarea_reconcile.js";
import {
  hostScrollState, restoreHostScroll, revealHostRange, setHostSelections,
} from "./host_navigation.js";
import { observeEditorResize } from "./resize_anchor.js";
import { directShortcutOperation, normalizeShortcutBindings, normalizeShortcutPolicy } from "./shortcuts.js";
import { EDITOR_STYLES } from "./styles.js";
import { installEditorListeners } from "./component_listeners.js";
import {
  beginComposition, commitComposition, endComposition, renderComposingPresentation,
} from "./component_composition.js";
import { isTouchScrolling } from "./scroll_surface.js";
import { raiseSoftKeyboard } from "./soft_keyboard.js";
const HTMLElementBase = globalThis.HTMLElement ?? class {};
const EVENT_VERSION = 1;
/** Framework-neutral browser editor backed by the shared synchronous engine. */
export class ContinuityEditorElement extends HTMLElementBase {
  static get observedAttributes() {
    return ["aria-label", "command-rail", "indent-guides", "rail-storage-key", "readonly", "shortcut-policy", "spellcheck", "syntax", "tab-behavior", "theme"];
  }
  #abortController;
  #affordances;
  #carets;
  #collapseSelectionsOnPointer = false;
  #commandRail;
  #composedRunLength = 0;
  #deferredHostReplace = null;
  #editor;
  #emitters = createEditorEmitters(this);
  #frame;
  #initialRevision = 0;
  #initialValue;
  #input;
  #inputSynchronizer = new EditorInputSynchronizer();
  #nativeSelectionWatcher;
  #touchSelection = new TouchSelectionGesture();
  #autoScroller = new DragAutoScroller();
  #overlays;
  #inputWidth = 0;
  #inputSelectionKey = "";
  #isComposing = false;
  #isDestroyed = false;
  #isFocusEscapeArmed = false;
  #keyboardHelp;
  #pointerGesture = new ProjectionPointerGesture();
  #pointerContext;
  #railRegistry = createRailRegistry();
  #transfer;
  #viewport;
  #pendingEdits = [];
  #pendingPointerSelection = null;
  #projection;
  #ready;
  #shield;
  #surface;
  #rejectReady;
  #renderScheduler;
  #resizeObserver;
  #resolveReady;
  #isReadySettled = false;
  #shouldFocusWhenReady = false;
  #shortcutBindings = new Map();
  #started = false;
  constructor() {
    super();
    this.#initialValue = normalizeNewlines(this.getAttribute?.("value") ?? this.textContent ?? "");
    this.#ready = new Promise((resolve, reject) => {
      this.#resolveReady = resolve;
      this.#rejectReady = reject;
    });
  }

  /** Resolves when WASM initialization and the first visible projection finish. */
  get ready() {
    return this.#ready;
  }
  get value() {
    return this.#editor?.snapshot().text ?? this.#initialValue;
  }

  set value(nextValue) {
    if (!this.#editor) {
      this.#initialValue = normalizeNewlines(String(nextValue));
      return;
    }
    this.replaceValue(nextValue, this.#editor.snapshot().revision);
  }
  get initialRevision() {
    return this.#editor?.snapshot().revision ?? this.#initialRevision;
  }

  set initialRevision(value) {
    if (this.#editor) {
      throw new Error("initialRevision must be set before the editor is ready");
    }
    if (!Number.isSafeInteger(value) || value < 0) {
      throw new RangeError("initialRevision must be a non-negative safe integer");
    }
    this.#initialRevision = value;
  }
  get readOnly() {
    return this.hasAttribute("readonly");
  }

  set readOnly(value) {
    this.toggleAttribute("readonly", Boolean(value));
  }
  get shortcutPolicy() {
    return normalizeShortcutPolicy(this.getAttribute("shortcut-policy"));
  }

  set shortcutPolicy(value) {
    this.setAttribute("shortcut-policy", normalizeShortcutPolicy(value));
  }
  get shortcutBindings() {
    return new Map(this.#shortcutBindings);
  }

  set shortcutBindings(value) {
    this.#shortcutBindings = normalizeShortcutBindings(value);
  }
  get syntax() { return this.getAttribute("syntax") === "plain" ? "plain" : "markdown"; }
  set syntax(value) { this.setAttribute("syntax", value === "plain" ? "plain" : "markdown"); }
  get indentGuides() { return this.getAttribute("indent-guides") === "on" ? "on" : "off"; }
  set indentGuides(value) { this.setAttribute("indent-guides", value === "on" ? "on" : "off"); }
  connectedCallback() {
    if (!this.#started && !this.#isDestroyed) {
      this.#started = true;
      this.#start();
    }
  }
  disconnectedCallback() {
    queueMicrotask(() => {
      if (!this.isConnected) {
        this.destroy();
      }
    });
  }
  attributeChangedCallback(name) {
    this.#applyAttributes();
    if (name === "syntax" || name === "indent-guides") {
      this.#renderScheduler?.schedule(performance.now());
    }
  }
  /** Return a complete immutable engine snapshot. */
  snapshot() {
    return { ...this.#requireEditor().snapshot(), isReadOnly: this.readOnly };
  }
  get composing() { return this.#isComposing; }
  /** Replace the complete document only when the host revision is current. */
  replaceValue(value, expectedRevision, timestampMs = Date.now()) {
    const normalized = normalizeNewlines(String(value));
    if (this.#isComposing && this.#editor) {
      this.#deferredHostReplace = { value: normalized, revision: expectedRevision };
      return null;
    }
    const startedAt = performance.now();
    const change = this.#requireEditor().replaceValue(normalized, expectedRevision, timestampMs);
    this.#commitChange(change, startedAt, "hostReplacement");
    return change;
  }
  /**
   * Insert text at the current selection. How a host completes a `pasteText`
   * request: the browser can block `clipboard.readText()` under a permissions
   * policy (an iframe without `allow="clipboard-read"`), which no editor-side
   * code can lift, so a host with another clipboard route hands the text here.
   */
  insertText(text) {
    raiseSoftKeyboard(this.#input);
    return this.#applyOperation("insert", String(text));
  }
  /** Execute one storage-neutral editor command synchronously. */
  executeCommand(command, timestampMs = Date.now()) {
    return this.#applyOperation("command", String(command), timestampMs);
  }
  /** Add one host action to the command rail; the result unregisters it. */
  registerRailAction(action) {
    return this.#railRegistry.register(action);
  }
  get railActions() { return this.#railRegistry.list(); }
  /** Replace every host action on the rail; built-ins are never affected. */
  set railActions(actions) { this.#railRegistry.replaceAll(actions); }
  /** Replace all engine selections and optionally reveal the primary selection. */
  setSelections(selections, options = {}) {
    return setHostSelections(this.#pointerContext, this.#requireEditor(), selections, options);
  }
  /** Reveal a source range and make it the primary selection. */
  revealRange(range, options = {}) {
    revealHostRange(this.#pointerContext, this.#requireEditor(), range, options);
  }
  /** Paint one named set of source ranges; an empty list removes the set. */
  setDecorations(id, ranges) { return this.#overlays.setDecorations(id, ranges); }
  /** Remove one decoration set, or every set when no id is given. */
  clearDecorations(id) { this.#overlays.clearDecorations(id); }
  /** Serializable undo history, for restoring across a remount. */
  exportHistory(options) { return this.#requireEditor().exportHistory(options); }
  /** Adopt history captured by `exportHistory` for this same document state. */
  importHistory(history) { this.#requireEditor().importHistory(history); }
  /**
   * Inclusive source-line window the reader can currently see, or `null`
   * before the first layout and while the host keeps the editor hidden.
   */
  visibleLineRange() { return this.#viewport?.read() ?? null; }
  /** Capture the viewport for per-document restoration. */
  getScrollState() { return hostScrollState(this.#pointerContext); }
  /** Restore a captured viewport. */
  restoreScrollState(s) { restoreHostScroll(this.#pointerContext, this.#requireEditor().snapshot(), s); }
  /** Report current shared WebAssembly linear memory. */
  linearMemoryBytes() { return this.#requireEditor().linearMemoryBytes(); }
  /** Focus the semantic browser input surface. */
  focus(options) {
    if (this.#input) {
      this.#input.focus(options);
    } else {
      this.#shouldFocusWhenReady = true;
    }
  }

  /** Idempotently release the WASM editor and every browser listener. */
  destroy() {
    if (this.#isDestroyed) return;
    this.#isDestroyed = true;
    this.#pendingPointerSelection = null;
    this.#nativeSelectionWatcher?.destroy();
    if (this.#projection) clearProjectionCompositionLine(this.#projection);
    this.#abortController?.abort();
    this.#resizeObserver?.disconnect();
    this.#renderScheduler?.destroy();
    this.#viewport?.destroy();
    if (!this.#isReadySettled) {
      this.#isReadySettled = true;
      this.#rejectReady(new Error("Continuity editor was destroyed before becoming ready"));
    }
    this.#editor?.destroy();
    this.#editor = undefined;
    dispatchEditorEvent(this, "continuity-destroy", { version: EVENT_VERSION });
  }

  async #start() {
    try {
      this.#buildShadowTree();
      await initialize();
      if (this.#isDestroyed) {
        return;
      }
      this.#editor = new Editor(this.#initialValue, this.#initialRevision);
      this.#renderScheduler = new RenderScheduler({
        canRender: () => Boolean(this.#editor && !this.#isDestroyed && !this.#isComposing),
        renderFast: () => this.#renderFast(),
        renderFull: () => this.#render(),
        onFrame: (inputStartedAt, paintedAt) => {
          // Every cause of movement lands here or in `#onScroll`: an edit that
          // reflows content under a stationary offset, a programmatic reveal,
          // and resize/zoom/font changes all reach a render first.
          this.#viewport?.schedule();
          dispatchEditorFrame(this, this.#editor.snapshot().revision, inputStartedAt, paintedAt);
        },
      });
      this.#syncInputFromEngine();
      this.#render();
      this.#isReadySettled = true;
      this.#resolveReady(this);
      if (this.#shouldFocusWhenReady) {
        this.#input.focus();
      }
      dispatchEditorEvent(this, "continuity-ready", {
        version: EVENT_VERSION,
        snapshot: this.snapshot(),
      });
      // Seeded after `continuity-ready` so a host that subscribes in its ready
      // handler still receives the opening window rather than having to poll.
      this.#viewport.publishNow();
    } catch (error) {
      if (!this.#isReadySettled) {
        this.#isReadySettled = true;
        this.#rejectReady(error);
      }
      this.#emitError(error);
    }
  }

  #buildShadowTree() {
    const dom = buildEditorDom(this, EDITOR_STYLES);
    this.#affordances = dom.affordances;
    this.#carets = dom.carets;
    this.#frame = dom.frame;
    this.#input = dom.input;
    this.#keyboardHelp = dom.keyboardHelp;
    this.#projection = dom.projection;
    this.#shield = dom.shield;
    this.#surface = {
      input: dom.input, projection: dom.projection, affordances: dom.affordances,
      carets: dom.carets, frame: dom.frame, shield: dom.shield, shieldSpacer: dom.shieldSpacer,
      onSelectionPainted: () => this.#overlays?.update(),
    };
    this.#inputSynchronizer.configure(this.#surface);
    this.#clipboardHooks = createClipboardHooks({
      input: this.#input, emitRequest: (k, p) => this.#emitRequest(k, p), emitError: (e) => this.#emitError(e),
    });

    this.#pointerContext = {
      ...this.#surface,
      gesture: this.#pointerGesture, touchSelection: this.#touchSelection,
      autoScroller: this.#autoScroller,
      scroller: () => this.#inputSynchronizer.scroller, editor: () => this.#editor,
      isComposing: () => this.#isComposing,
      getPending: () => this.#pendingPointerSelection, setPending: (v) => { this.#pendingPointerSelection = v; },
      getCollapse: () => this.#collapseSelectionsOnPointer, setCollapse: (v) => { this.#collapseSelectionsOnPointer = v; },
      renderComposing: () => this.#renderComposingPresentation(), commitComposition: () => this.#commitComposition(),
      getSelectionKey: () => this.#inputSelectionKey, setSelectionKey: (k) => { this.#inputSelectionKey = k; },
      surface: () => (isTouchScrolling() ? this.#shield : this.#input),
      sync: (reveal) => this.#syncInputFromEngine(reveal), schedule: () => this.#scheduleRender(performance.now()),
      requestReveal: (range, align) => this.#inputSynchronizer.requestReveal(range, align),
      syncScroll: () => this.#inputSynchronizer.onScroll(this.#editor.snapshot()),
      probeViewport: () => this.#viewport?.schedule(),
      emitRequest: (kind, payload) => this.#emitRequest(kind, payload), executeCommand: (c) => this.executeCommand(c),
      setComposing: (v) => { this.#isComposing = v; }, composedRunLength: () => this.#composedRunLength,
      setComposedRunLength: (v) => { this.#composedRunLength = v; }, isReadOnly: () => this.readOnly,
      commitChange: (change, source, sync) => this.#commitChange(change, performance.now(), source, sync),
      getDeferredReplace: () => this.#deferredHostReplace, setDeferredReplace: (v) => { this.#deferredHostReplace = v; },
      insertText: (text) => this.#applyOperation("insert", text),
      writeClipboard: (t) => copyText(t, this.#clipboardHooks), readClipboard: () => readClipboardText(this.#clipboardHooks),
      updateSelectionActions: () => this.#overlays?.update(),
      offerSelectionActionsForCaret: () => this.#overlays?.offerForCaret(),
      hideSelectionActions: () => this.#overlays?.hide(),
    };

    this.#transfer = createTransferHandlers(this.#pointerContext, this);
    this.#viewport = createViewportWatch(this.#pointerContext, (r) => dispatchEditorViewport(this, r));
    this.#overlays = createEditorOverlays(this.#pointerContext, this.#surface);
    this.#applyTouchScrolling();
    this.#installListeners();
    this.#commandRail = attachCommandRail(this, dom, {
      registry: this.#railRegistry,
      editorElement: this,
      execute: (command) => this.#executeShortcut(command),
      runOperation: (operation) => this.#runRailOperation(operation),
      emitRequest: (kind, payload) => this.#emitRequest(kind, payload),
      snapshot: () => (this.#editor ? this.snapshot() : null),
      reportError: (error) => this.#emitError(error),
    }, this.#abortController.signal);
    this.#applyAttributes();
  }

  #installListeners() {
    this.#abortController = new AbortController();
    const options = { signal: this.#abortController.signal };
    this.#nativeSelectionWatcher = new NativeSelectionWatcher({
      input: this.#input,
      canAdopt: () => Boolean(this.#editor) && !this.#isComposing && !this.#isDestroyed,
      currentKey: () => this.#inputSelectionKey,
      adopt: () => this.#onSelection(),
    });
    installEditorListeners(this.#surface, {
      onBeforeInput: this.#onBeforeInput,
      onInput: this.#onInput,
      onCompositionStart: this.#onCompositionStart,
      onCompositionEnd: this.#onCompositionEnd,
      onKeyDown: this.#onKeyDown,
      onSelectStart: this.#onSelectStart,
      onSelection: this.#onSelection,
      onSelectionChange: this.#nativeSelectionWatcher.onSelectionChange,
      onScroll: this.#onScroll,
      onPaste: this.#transfer.onPaste,
      onCut: this.#transfer.onCut,
      onDrop: this.#transfer.onDrop,
      onPointerDown: this.#onPointerDown,
      onPointerMove: this.#onPointerMove,
      onPointerCancel: this.#onPointerCancel,
      onPointerUp: this.#onPointerUp,
      onClick: this.#onClick,
      onContextMenu: this.#transfer.onContextMenu,
      onTouchPointerChange: this.#applyTouchScrolling,
      onTouchMove: this.#onTouchMove,
    }, {
      snapshot: () => this.#editor?.snapshot(),
    }, options);
    this.#resizeObserver = observeEditorResize(this, this.#input, this.#frame, {
      hasEditor: () => Boolean(this.#editor),
      inputWidth: () => this.#inputWidth,
      scheduleRender: (startedAt) => this.#scheduleRender(startedAt),
      scrollTop: () => this.#inputSynchronizer.scrollTop,
      setInputWidth: (width) => { this.#inputWidth = width; },
      setScrollTop: (scrollTop) => { this.#inputSynchronizer.scrollTop = scrollTop; },
    });
  }

  #onBeforeInput = (event) => {
    handleBeforeInput(event, {
      isReadOnly: this.readOnly,
      isComposing: this.#isComposing,
      applyOperation: (kind, value) => this.#applyOperation(kind, value),
      commitComposition: () => this.#commitComposition(),
    });
  };

  #onInput = (event) => {
    if (this.#isComposing) {
      // Preview the live composing line only; the engine stays untouched.
      this.#composedRunLength = event?.data?.length ?? 0;
      this.#renderComposingPresentation();
      return;
    }
    if (!this.#editor) return;
    const change = reconcileTextareaMutation(this.#editor, this.#input);
    if (change) {
      // Textarea already holds the mutated text; sync without re-applying edits.
      this.#commitChange(change, performance.now(), "browserReconcile", null);
    }
  };

  #renderComposingPresentation() { renderComposingPresentation(this.#pointerContext); }

  #onCompositionStart = () => beginComposition(this.#pointerContext);

  #commitComposition = () => commitComposition(this.#pointerContext);

  #onCompositionEnd = () => endComposition(this.#pointerContext);

  #onKeyDown = (event) => {
    this.#isFocusEscapeArmed = handleEditorKeyDown(event, {
      executeCommand: (command) => this.#executeShortcut(command),
      isComposing: this.#isComposing,
      isFocusEscapeArmed: this.#isFocusEscapeArmed,
      hasMultipleSelections: this.#editor?.snapshot().selections.length > 1,
      isReadOnly: this.readOnly,
      addVerticalCaret: (direction) => applyVerticalCaret(this.#pointerContext, direction),
      clearSecondarySelections: () => clearSecondarySelections(this.#pointerContext),
      insertRawNewline: () => this.#applyOperation("insert", "\n"),
      onShortcutSuppressed: (detail) => dispatchShortcutSuppressed(this, detail),
      shortcutBindings: this.#shortcutBindings,
      shortcutPolicy: this.shortcutPolicy,
      tabBehavior: this.getAttribute("tab-behavior"),
    });
    if (!event.defaultPrevented && /^(?:Arrow|Home|End|Page)/u.test(event.key)) requestAnimationFrame(this.#onSelection);
  };
  #executeShortcut(command) {
    const directOperation = directShortcutOperation(command);
    return directOperation ? this.#applyOperation(directOperation) : this.executeCommand(command);
  }

  #onSelection = () => {
    applyNativeSelection(this.#pointerContext);
    // Rail enablement predicates read the selection, not only the text.
    this.#commandRail?.refresh();
  };
  #onPointerDown = (event) => {
    // Only a press on the editing surface dismisses the bar. A press on the bar
    // itself bubbles here too, and hiding it mid-tap sets `display: none`, so
    // the button is gone before the click can land — a dead button.
    if (!isSurfaceEvent(this.#pointerContext, event)) return;
    this.#overlays?.hide();
    handlePointerDown(this.#pointerContext, event);
  };

  #onPointerMove = (event) => routeSurfacePointer(this.#pointerContext, event, handlePointerMove);

  #onPointerUp = (event) => routeSurfacePointer(this.#pointerContext, event, handlePointerUp);

  #onPointerCancel = () => handlePointerCancel(this.#pointerContext);
  #onSelectStart = (e) => handleSelectStart(this.#pointerContext, e);
  #onTouchMove = (e) => handleTouchMove(this.#pointerContext, e);

  // The class drives the stylesheet and the same predicate picks the scroller in
  // JS, so the two can never disagree about which element owns the touch.
  #applyTouchScrolling = () => {
    this.#frame.classList.toggle("touch-scrolling", isTouchScrolling());
    this.#scheduleRender(performance.now());
  };

  #onScroll = () => handleEditorScroll(this.#pointerContext);

  #onClick = (event) => routeSurfacePointer(
    this.#pointerContext, event, completeProjectedPointerClick,
  );

  // Caret motion is measured against wrapped browser layout, so it has no
  // storage-neutral engine command; the rail routes these operations here.
  #runRailOperation(operation) {
    if (operation === "caretUp") moveVerticalCaret(this.#pointerContext, -1);
    if (operation === "caretDown") moveVerticalCaret(this.#pointerContext, 1);
  }

  #applyOperation(kind, value, timestamp = Date.now()) {
    if (this.readOnly || !this.#editor) return null;
    const startedAt = performance.now();
    const selectionKey = inputSelectionKey(this.#input);
    if (selectionKey !== this.#inputSelectionKey) {
      this.#editor.setSelections(selectionsWithInputPrimary(this.#editor.snapshot(), this.#input));
      this.#inputSelectionKey = selectionKey;
    }
    try {
      const change = applyEditorOperation(this.#editor, kind, value, timestamp);
      this.#commitChange(change, startedAt, kind === "command" ? `command:${value}` : kind);
      return change;
    } catch (error) {
      this.#emitError(error);
      throw error;
    }
  }

  #commitChange(change, startedAt, source, syncChange = change) {
    if (!this.#editor) return;
    this.#syncInputFromEngine(source !== "hostReplacement", syncChange);
    this.#pendingEdits.push(...(change?.edits ?? []));
    if (change) {
      this.#emitters.emitChange({
        source,
        commitOrigin: source === "hostReplacement" ? "host" : "user",
        change,
        snapshot: this.snapshot(),
      });
    }
    this.#scheduleRender(startedAt);
  }

  #syncInputFromEngine(shouldRevealCaret = false, change = null) {
    const snapshot = this.#editor.snapshot();
    this.#inputSelectionKey = this.#inputSynchronizer.apply(snapshot, change, shouldRevealCaret);
    this.#inputSynchronizer.onScroll(snapshot);
  }

  #scheduleRender(startedAt) {
    this.#renderScheduler?.schedule(startedAt);
  }

  #renderFast() {
    const rendered = renderFastPresentation({
      snapshot: this.#editor.snapshot(), input: this.#input, projection: this.#projection,
      affordances: this.#affordances, carets: this.#carets, synchronizer: this.#inputSynchronizer,
      edits: this.#pendingEdits.splice(0),
      shouldFreezeSourceReveal: this.#shouldFreezeSourceReveal,
    });
    if (!rendered) this.#render();
  }

  #shouldFreezeSourceReveal = () => this.#touchSelection.isActive
    || this.#overlays?.isAdjusting() === true;

  #render() {
    // The idle full pass can re-project lines the fast path left coarse, which
    // changes their height and so the window; it reaches no `onFrame`.
    this.#viewport?.schedule();
    renderEditorPresentation({
      editor: this.#editor, syntax: this.syntax, input: this.#input,
      projection: this.#projection, affordances: this.#affordances, carets: this.#carets,
      synchronizer: this.#inputSynchronizer, copyCode: (text) => copyCodeBlock(text, this.#clipboardHooks),
      shouldFreezeSourceReveal: this.#shouldFreezeSourceReveal,
    });
  }

  #clipboardHooks;

  #applyAttributes() {
    applyEditorAttributes(
      this, this.#input, this.#keyboardHelp, keyboardHelpText, this.#projection,
    );
    this.#commandRail?.update();
  }

  #emitRequest(kind, payload) { this.#emitters.emitRequest(kind, payload); }

  #emitError(error) { this.#emitters.emitError(error); }

  #requireEditor() {
    return requireLiveEditor(this.#editor, this.#isDestroyed);
  }
}
