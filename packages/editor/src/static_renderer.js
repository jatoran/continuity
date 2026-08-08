import { Editor, initialize } from "./engine.js";
import { createPlainPresentation, renderProjection, renderProjectionViewport } from "./projection.js";
import { EDITOR_STYLES } from "./styles.js";

const HTMLElementBase = globalThis.HTMLElement ?? class {};
const STATIC_STYLES = `${EDITOR_STYLES}
:host { min-block-size: 0; }
.static-viewport { box-sizing: border-box; overflow: auto; width: 100%; height: 100%; }
.static-viewport .projection {
  position: relative; inset: auto; width: 100%; min-height: 100%;
  pointer-events: auto; user-select: text; will-change: auto;
}
`;

/** Lightweight selectable Markdown projection without an editing textarea. */
export class ContinuityRendererElement extends HTMLElementBase {
  static get observedAttributes() { return ["aria-label", "syntax", "theme"]; }
  #editor;
  #initialValue;
  #isDestroyed = false;
  #projection;
  #ready;
  #scroll;

  constructor() {
    super();
    this.#initialValue = String(this.getAttribute?.("value") ?? this.textContent ?? "");
    this.#ready = this.#start();
  }

  get ready() { return this.#ready; }
  get value() { return this.#editor?.snapshot().text ?? this.#initialValue; }
  set value(value) {
    const text = String(value).replaceAll("\r\n", "\n").replaceAll("\r", "\n");
    if (!this.#editor) this.#initialValue = text;
    else {
      const snapshot = this.#editor.snapshot();
      this.#editor.replaceValue(text, snapshot.revision);
      this.#render();
    }
  }
  get syntax() { return this.getAttribute("syntax") === "plain" ? "plain" : "markdown"; }
  set syntax(value) { this.setAttribute("syntax", value === "plain" ? "plain" : "markdown"); }
  connectedCallback() { this.#ready.catch((error) => this.#emitError(error)); }
  disconnectedCallback() { queueMicrotask(() => { if (!this.isConnected) this.destroy(); }); }
  attributeChangedCallback() { if (this.#editor) this.#render(); }
  getScrollState() { return { top: this.#scroll?.scrollTop ?? 0, left: this.#scroll?.scrollLeft ?? 0 }; }
  restoreScrollState(state) {
    if (!this.#scroll) return;
    this.#scroll.scrollTop = Number.isFinite(state?.top) ? Math.max(0, state.top) : 0;
    this.#scroll.scrollLeft = Number.isFinite(state?.left) ? Math.max(0, state.left) : 0;
    renderProjectionViewport(this.#projection, this.#scroll);
  }
  linearMemoryBytes() { return this.#editor?.linearMemoryBytes() ?? 0; }
  destroy() { this.#isDestroyed = true; this.#editor?.destroy(); this.#editor = undefined; }

  async #start() {
    if (!globalThis.document) return this;
    const shadow = this.attachShadow({ mode: "open" });
    const style = document.createElement("style");
    style.textContent = STATIC_STYLES;
    this.#scroll = document.createElement("div");
    this.#scroll.className = "static-viewport";
    this.#scroll.setAttribute("part", "viewport");
    this.#scroll.setAttribute("role", "document");
    this.#projection = document.createElement("div");
    this.#projection.className = "projection static-projection";
    this.#projection.setAttribute("part", "projection");
    this.#scroll.append(this.#projection);
    shadow.append(style, this.#scroll);
    this.#scroll.addEventListener("scroll", () => renderProjectionViewport(this.#projection, this.#scroll), { passive: true });
    await initialize();
    if (this.#isDestroyed) return this;
    this.#editor = new Editor(this.#initialValue);
    this.#render();
    return this;
  }

  #render() {
    const snapshot = this.#editor.snapshot();
    const presentation = this.syntax === "plain"
      ? createPlainPresentation(snapshot.text)
      : this.#editor.presentation();
    this.#scroll.setAttribute("aria-label", this.getAttribute("aria-label") || "Rendered Markdown");
    renderProjection(this.#projection, this.#scroll, snapshot, presentation, new Set());
  }

  #emitError(error) {
    this.dispatchEvent(new CustomEvent("continuity-error", {
      bubbles: true, composed: true, detail: { version: 1, error },
    }));
  }
}
