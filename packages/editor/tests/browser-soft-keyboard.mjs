// Soft-keyboard gate contract.
//
// On Android the keyboard is not a function of DOM focus. The system back
// gesture hides the IME without blurring, so the textarea is still focused
// afterwards, and Chrome re-raises the keyboard for the next touch that resolves
// against it — including a long-press that only meant to select. The editor
// answers with `inputmode="none"`, held by default on a coarse pointer and
// lifted only where a touch has already been resolved as typing.
//
// The attribute is not symmetric, which is the second contract here: applying it
// to a field that is focused *with the IME up* dismisses that keyboard rather
// than refusing a future one. So the gate consults tracked keyboard state, and a
// selection gesture closes it only where there is no keyboard to lose.
//
// What can be asserted here is the state machine, not the platform: a headless
// browser has no IME to raise. Every assertion is therefore about the attribute
// Chrome reads when it decides, plus the focus re-entry that re-arms its request
// — which is exactly the pair that has to be right for the phone to behave. The
// raised-keyboard half is driven through an injected visual viewport, because
// viewport occlusion is the signal the phone itself uses to report an IME.

import { animationFrames, glyphRect, settle } from "./browser-touch-helpers.mjs";

const SOURCE = "alpha beta gamma delta epsilon zeta\nsecond line of prose here\n";

export async function runSoftKeyboardGateTests(ContinuityEditorElement, check) {
  if (!matchMedia("(pointer: coarse)").matches) {
    return runFinePointerCase(ContinuityEditorElement, check);
  }
  let assertions = await runCoarsePointerCases(ContinuityEditorElement, check);
  assertions += await runRaisedKeyboardCases(ContinuityEditorElement, check);
  return assertions;
}

/**
 * Desktop is untouched by the gate. Mouse and pen already focus on pointerdown
 * and `inputmode` means nothing to a physical keyboard, so the attribute must
 * never appear on a fine pointer — including on a touchscreen laptop, where it
 * would suppress the platform's on-screen keyboard for no reason.
 */
async function runFinePointerCase(ContinuityEditorElement, check) {
  const { editor, input, projection, dispose } = await mount(ContinuityEditorElement);
  let assertions = 0;
  check(!input.hasAttribute("inputmode"),
    "a fine-pointer editor never gates its input mode"); assertions += 1;
  editor.insertText("x");
  await settle();
  check(!input.hasAttribute("inputmode"),
    "an insert on a fine pointer leaves the input mode alone"); assertions += 1;

  // The raise-only tap is a phone affordance and must not reach a mouse: a click
  // inside a selection places a caret there, as it always has.
  editor.setSelections([{
    anchor: { line: 0, byteInLine: 6 }, head: { line: 0, byteInLine: 10 }, kind: "caret",
  }], { reveal: false });
  await settle();
  const line = projection.children[0];
  const rect = glyphRect(line, line.textContent.indexOf("beta") + 1);
  const clickX = rect.left + rect.width / 2;
  const clickY = rect.top + rect.height / 2;
  for (const type of ["pointerdown", "pointerup"]) {
    input.dispatchEvent(new PointerEvent(type, {
      bubbles: true, cancelable: true, pointerType: "mouse", pointerId: 3, isPrimary: true,
      button: 0, buttons: type === "pointerdown" ? 1 : 0, clientX: clickX, clientY: clickY,
    }));
  }
  input.dispatchEvent(new PointerEvent("click", {
    bubbles: true, cancelable: true, pointerType: "mouse", pointerId: 3,
    button: 0, buttons: 0, clientX: clickX, clientY: clickY, detail: 1,
  }));
  await settle();
  check(input.selectionStart === input.selectionEnd,
    "a mouse click inside a selection still collapses it"); assertions += 1;
  check(!input.hasAttribute("inputmode"),
    "a mouse click inside a selection leaves the input mode alone"); assertions += 1;
  dispose();
  return assertions;
}

async function runCoarsePointerCases(ContinuityEditorElement, check) {
  const { editor, input, projection, shield, shadow, dispose } = await mount(
    ContinuityEditorElement,
  );
  let assertions = 0;
  const touch = (type, x, y, extra = {}) => shield.dispatchEvent(new PointerEvent(type, {
    bubbles: true, cancelable: true, pointerType: "touch", pointerId: 57,
    isPrimary: true, button: 0, buttons: 1, clientX: x, clientY: y, ...extra,
  }));
  const tap = (x, y) => {
    touch("pointerdown", x, y);
    touch("pointerup", x, y, { buttons: 0 });
    touch("click", x, y, { buttons: 0, detail: 1 });
  };
  const isGated = () => input.getAttribute("inputmode") === "none";

  check(isGated(), "a coarse-pointer editor mounts with the keyboard gate closed");
  assertions += 1;

  // Re-read the line on every lookup: a repaint replaces the element, and the
  // caret line reveals its source, so coordinates captured once go stale.
  const at = (word, offset) => {
    const element = projection.children[0];
    const rect = glyphRect(element, element.textContent.indexOf(word) + offset);
    return [rect.left + rect.width / 2, rect.top + rect.height / 2];
  };

  // --- A resolved tap is the one touch that means "type here". ---
  const [tapX, tapY] = at("gamma", 2);
  tap(tapX, tapY);
  await settle();
  check(!input.hasAttribute("inputmode"), "a resolved tap opens the gate"); assertions += 1;
  check(shadow.activeElement === input, "a resolved tap focuses the input"); assertions += 1;

  // --- The reported state: keyboard dismissed by the back gesture, which
  // leaves focus exactly where it was. A long-press from here must not raise it.
  check(shadow.activeElement === input,
    "dismissing the keyboard would leave the input focused"); assertions += 1;

  touch("pointerdown", tapX, tapY);
  await new Promise((resolve) => setTimeout(resolve, 420));
  await settle();
  check(isGated(), "a long-press closes the gate before the finger lifts"); assertions += 1;
  check(input.value.slice(input.selectionStart, input.selectionEnd) === "gamma",
    `the long-press still selects the projected word (got ${JSON.stringify(input.value.slice(input.selectionStart, input.selectionEnd))})`);
  assertions += 1;

  const [dragX, dragY] = at("epsilon", 6);
  touch("pointermove", dragX, dragY);
  await settle();
  check(isGated(), "extending the selection keeps the gate closed"); assertions += 1;
  touch("pointerup", dragX, dragY, { buttons: 0 });
  await settle();
  // Some engines synthesize a click once the finger lifts. It resolves against a
  // still-focused textarea, which is the exact shape of the reported defect.
  touch("click", dragX, dragY, { buttons: 0, detail: 1 });
  check(isGated(), "a trailing click after a long-press cannot open the gate"); assertions += 1;
  check(shadow.activeElement === input,
    "the selection gesture never moved focus, so no focus policy could have gated it");
  assertions += 1;

  // --- The tap that follows must still raise the keyboard. Focus never left, so
  // `focus()` alone is a no-op; re-entering it is what re-arms Chrome's request.
  let focusEvents = 0;
  const countFocus = () => { focusEvents += 1; };
  input.addEventListener("focus", countFocus);
  tap(tapX, tapY);
  await settle();
  check(!input.hasAttribute("inputmode"), "a tap after a selection re-opens the gate");
  assertions += 1;
  check(focusEvents === 1,
    `the tap re-enters focus so the browser re-arms the IME request (${focusEvents} focus event(s))`);
  assertions += 1;
  input.removeEventListener("focus", countFocus);

  // --- Grabbing an adjust handle is selection, not typing. Seeded through the
  // host API so the gate is open going in and the assertion cannot pass vacuously.
  editor.setSelections([{
    anchor: { line: 0, byteInLine: 0 }, head: { line: 0, byteInLine: 5 }, kind: "caret",
  }], { reveal: false });
  await settle();
  check(!input.hasAttribute("inputmode"),
    "a host selection change leaves the gate as the last touch left it"); assertions += 1;
  const handle = shadow.querySelector('.selection-handle[data-selection-edge="end"]');
  check(Boolean(handle) && !handle.hidden, "a settled selection offers adjust handles");
  assertions += 1;
  const handleBounds = handle.getBoundingClientRect();
  handle.dispatchEvent(new PointerEvent("pointerdown", {
    bubbles: true, cancelable: true, pointerType: "touch", pointerId: 58,
    isPrimary: true, button: 0, buttons: 1,
    clientX: handleBounds.left + handleBounds.width / 2,
    clientY: handleBounds.top + handleBounds.height / 2,
  }));
  await animationFrames(2);
  check(isGated(), "grabbing an adjust handle closes the gate"); assertions += 1;

  // --- Typing intent expressed through a button still raises the keyboard. This
  // is the path the host rail's paste action takes; the editor's own paste
  // button reaches the same lift.
  editor.insertText("!");
  await settle();
  check(!input.hasAttribute("inputmode"), "a host insert opens the gate"); assertions += 1;
  check(editor.value.includes("!"), "a host insert still inserts"); assertions += 1;
  releaseHandle(handle, handleBounds);
  await settle();

  // --- A selection made with the keyboard down needs a tap to raise it, and
  // that tap cannot also be the tap that destroys the selection: every route to
  // acting on the selection — typing over it, the whole action bar — needs the
  // keyboard, so collapsing on the way to it left nothing to act on.
  const selectedText = () => input.value.slice(input.selectionStart, input.selectionEnd);
  const [betaX, betaY] = at("beta", 1);
  await longPressAt(touch, betaX, betaY);
  check(selectedText() === "beta",
    `a long-press selects the projected word (got ${JSON.stringify(selectedText())})`);
  assertions += 1;
  check(isGated(), "the selection gesture left the keyboard down"); assertions += 1;

  const actions = shadow.querySelector(".selection-actions");
  tap(betaX, betaY);
  await settle();
  check(!isGated(), "the first tap inside a selection raises the keyboard"); assertions += 1;
  check(selectedText() === "beta",
    `the raising tap preserves the whole selection (got ${JSON.stringify(selectedText())})`);
  assertions += 1;
  check(Boolean(actions) && !actions.hidden,
    "the selection actions survive the raising tap"); assertions += 1;
  check([...actions.querySelectorAll("[data-selection-action]")]
    .some((button) => button.dataset.selectionAction === "copy" && !button.hidden),
  "copy is still offered after the raising tap"); assertions += 1;

  // --- The tap after it means what a tap normally means, or a reader could
  // never tap their way out of a selection they no longer want.
  tap(betaX, betaY);
  await settle();
  check(input.selectionStart === input.selectionEnd,
    `the next tap collapses the selection (got ${JSON.stringify(selectedText())})`);
  assertions += 1;

  // --- Outside the highlight nothing changes: an ordinary tap places a caret
  // and raises the keyboard, exactly as it did before any of this existed.
  await longPressAt(touch, betaX, betaY);
  check(selectedText() === "beta" && isGated(),
    "re-selecting puts the keyboard back down"); assertions += 1;
  const [zetaX, zetaY] = at("zeta", 1);
  tap(zetaX, zetaY);
  await settle();
  check(!isGated(), "a tap outside the selection still raises the keyboard"); assertions += 1;
  check(input.selectionStart === input.selectionEnd,
    `a tap outside the selection places a caret (got ${JSON.stringify(selectedText())})`);
  assertions += 1;

  dispose();
  return assertions;
}

/**
 * A keyboard that is already up must survive a selection.
 *
 * `inputmode="none"` on a focused field with the IME showing takes that IME
 * away, so the gate cannot be a reflex — it has to know whether a keyboard is
 * there to lose. A headless browser raises none, so the platform's own report is
 * injected: on Android an IME is visible precisely as a visual viewport that has
 * lost height, and it is restored when the back gesture takes the keyboard away
 * without touching focus.
 */
async function runRaisedKeyboardCases(ContinuityEditorElement, check) {
  const viewport = createFakeViewport(800);
  const owned = Object.getOwnPropertyDescriptor(window, "visualViewport");
  Object.defineProperty(window, "visualViewport", { configurable: true, value: viewport });
  let mounted;
  let assertions = 0;
  try {
    mounted = await mount(ContinuityEditorElement);
    assertions = await assertRaisedKeyboardSurvives(mounted, viewport, check);
  } finally {
    mounted?.dispose();
    if (owned) Object.defineProperty(window, "visualViewport", owned);
    else delete window.visualViewport;
  }
  return assertions;
}

async function assertRaisedKeyboardSurvives({ input, projection, shield, shadow }, viewport, check) {
  let assertions = 0;
  const touch = (type, x, y, extra = {}) => shield.dispatchEvent(new PointerEvent(type, {
    bubbles: true, cancelable: true, pointerType: "touch", pointerId: 61,
    isPrimary: true, button: 0, buttons: 1, clientX: x, clientY: y, ...extra,
  }));
  const isGated = () => input.getAttribute("inputmode") === "none";
  const at = (word, offset) => {
    const element = projection.children[0];
    const rect = glyphRect(element, element.textContent.indexOf(word) + offset);
    return [rect.left + rect.width / 2, rect.top + rect.height / 2];
  };

  const [tapX, tapY] = at("gamma", 2);
  touch("pointerdown", tapX, tapY);
  touch("pointerup", tapX, tapY, { buttons: 0 });
  touch("click", tapX, tapY, { buttons: 0, detail: 1 });
  await settle();
  check(!isGated(), "a resolved tap opens the gate"); assertions += 1;
  // The IME arrives: the visual viewport loses height, the layout viewport does
  // not, and nothing whatsoever happens to focus.
  await viewport.reportHeight(480);
  check(!isGated(), "the keyboard arriving leaves the gate open"); assertions += 1;

  // --- Selecting while typing must not take the keyboard away.
  const [pressX, pressY] = at("delta", 2);
  await longPressAt(touch, pressX, pressY, { release: false });
  check(!isGated(),
    "a long-press with the keyboard up leaves the gate open"); assertions += 1;
  check(input.value.slice(input.selectionStart, input.selectionEnd) === "delta",
    `the long-press still selects through the projection (got ${JSON.stringify(input.value.slice(input.selectionStart, input.selectionEnd))})`);
  assertions += 1;
  const [dragX, dragY] = at("epsilon", 6);
  touch("pointermove", dragX, dragY);
  await settle();
  check(!isGated(), "extending the selection leaves a raised keyboard alone"); assertions += 1;
  touch("pointerup", dragX, dragY, { buttons: 0 });
  await settle();
  check(!isGated(), "the finger lifting leaves a raised keyboard alone"); assertions += 1;

  // --- The adjust handle is the same gesture reached another way.
  const handle = shadow.querySelector('.selection-handle[data-selection-edge="end"]');
  check(Boolean(handle) && !handle.hidden,
    "a selection made with the keyboard up still offers adjust handles"); assertions += 1;
  const bounds = handle.getBoundingClientRect();
  handle.dispatchEvent(new PointerEvent("pointerdown", {
    bubbles: true, cancelable: true, pointerType: "touch", pointerId: 62,
    isPrimary: true, button: 0, buttons: 1,
    clientX: bounds.left + bounds.width / 2, clientY: bounds.top + bounds.height / 2,
  }));
  await animationFrames(2);
  check(!isGated(), "grabbing an adjust handle leaves a raised keyboard alone"); assertions += 1;
  releaseHandle(handle, bounds, 62);
  await animationFrames(2);

  // --- And the protection this gate was built for is untouched: the back
  // gesture blurs nothing, so the viewport returning is the only report of it.
  await viewport.reportHeight(800);
  check(isGated(), "a dismissed keyboard re-arms the gate"); assertions += 1;
  await longPressAt(touch, pressX, pressY);
  check(isGated(),
    "a long-press after a dismissal still cannot raise the keyboard"); assertions += 1;
  return assertions;
}

/** A visual viewport whose height the test moves, as an IME would. */
function createFakeViewport(height) {
  const viewport = new EventTarget();
  viewport.height = height;
  viewport.width = 400;
  viewport.offsetLeft = 0;
  viewport.offsetTop = 0;
  viewport.pageLeft = 0;
  viewport.pageTop = 0;
  viewport.scale = 1;
  viewport.reportHeight = async (next) => {
    viewport.height = next;
    viewport.dispatchEvent(new Event("resize"));
    await animationFrames(2);
  };
  return viewport;
}

/** Hold a finger past the long-press threshold and let the claim render. */
async function longPressAt(touch, clientX, clientY, { release = true } = {}) {
  touch("pointerdown", clientX, clientY);
  await new Promise((resolve) => setTimeout(resolve, 420));
  await settle();
  if (!release) return;
  touch("pointerup", clientX, clientY, { buttons: 0 });
  await settle();
}

function releaseHandle(handle, bounds, pointerId = 58) {
  handle.dispatchEvent(new PointerEvent("pointerup", {
    bubbles: true, cancelable: true, pointerType: "touch", pointerId,
    isPrimary: true, button: 0, buttons: 0,
    clientX: bounds.left + bounds.width / 2, clientY: bounds.top + bounds.height / 2,
  }));
}

async function mount(ContinuityEditorElement) {
  const editor = new ContinuityEditorElement();
  editor.setAttribute("aria-label", "Soft keyboard gate document");
  editor.value = SOURCE;
  document.querySelector("#mount").append(editor);
  await editor.ready;
  const shadow = editor.shadowRoot;
  await settle();
  return {
    editor,
    shadow,
    input: shadow.querySelector("textarea"),
    projection: shadow.querySelector(".projection"),
    shield: shadow.querySelector(".touch-shield"),
    dispose: () => { editor.destroy(); editor.remove(); },
  };
}
