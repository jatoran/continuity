import { applySoftKeyboardGate } from "./soft_keyboard.js";

/** Build the semantic input, visual projection, and non-semantic overlay layers. */
export function buildEditorDom(host, styles) {
  const shadow = host.attachShadow({ mode: "open", delegatesFocus: true });
  const style = document.createElement("style");
  style.textContent = styles;
  const frame = document.createElement("div");
  frame.className = "frame";
  frame.setAttribute("part", "frame");
  const projection = document.createElement("div");
  projection.className = "projection";
  projection.setAttribute("part", "projection");
  projection.setAttribute("aria-hidden", "true");
  const affordances = document.createElement("div");
  affordances.className = "affordances";
  affordances.setAttribute("part", "affordances");
  const carets = document.createElement("div");
  carets.className = "secondary-carets";
  carets.setAttribute("aria-hidden", "true");
  const input = document.createElement("textarea");
  input.className = "input";
  input.setAttribute("part", "input");
  input.setAttribute("aria-multiline", "true");
  input.setAttribute("autocomplete", "off");
  input.setAttribute("autocapitalize", "off");
  input.setAttribute("autocorrect", "off");
  input.setAttribute("aria-describedby", "continuity-keyboard-help");
  input.setAttribute("wrap", "soft");
  // Closed until a touch resolves as typing. On a phone the textarea stays
  // focused after the keyboard is dismissed with the back gesture, and a focused
  // editable is all Chrome needs to raise the IME again for the next touch —
  // including a long-press that only meant to select.
  applySoftKeyboardGate(input);
  // Touch surface. A finger resting on the textarea gets the platform's own
  // long-press selection, hit-tested against a layout that cannot match the
  // projection — and no CSS or event can refuse it on an editable element. So
  // on touch the finger lands here instead, on a plain non-editable div where
  // `user-select: none` does apply and this shield owns the scrolling.
  const shield = document.createElement("div");
  shield.className = "touch-shield";
  shield.setAttribute("aria-hidden", "true");
  const shieldSpacer = document.createElement("div");
  shieldSpacer.className = "touch-shield-spacer";
  shield.append(shieldSpacer);
  frame.append(projection, input, shield, carets, affordances);
  const keyboardHelp = document.createElement("span");
  keyboardHelp.id = "continuity-keyboard-help";
  keyboardHelp.className = "keyboard-help";
  shadow.append(style, frame, keyboardHelp);
  return { affordances, carets, frame, input, keyboardHelp, projection, shield, shieldSpacer };
}

/** Synchronize host attributes into the semantic textarea and the projection. */
export function applyEditorAttributes(host, input, keyboardHelp, helpText, projection) {
  if (!input) {
    return;
  }
  input.readOnly = host.readOnly;
  input.setAttribute("aria-readonly", String(host.readOnly));
  input.setAttribute("aria-label", host.getAttribute("aria-label") || "Markdown editor");
  input.spellcheck = host.spellcheck;
  keyboardHelp.textContent = helpText(host.getAttribute("tab-behavior"), host.readOnly);
  // The projection carries the flag rather than the host, so the stylesheet's
  // gate and the per-line measurement read the same element.
  if (projection) {
    projection.dataset.indentGuides = host.getAttribute("indent-guides") === "on" ? "on" : "off";
  }
}
