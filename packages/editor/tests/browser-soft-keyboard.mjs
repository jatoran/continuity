// Soft-keyboard gate contract.
//
// On Android the keyboard is not a function of DOM focus. The system back
// gesture hides the IME without blurring, so the textarea is still focused
// afterwards, and Chrome re-raises the keyboard for the next touch that resolves
// against it — including a long-press that only meant to select. The editor
// answers with `inputmode="none"`, held by default on a coarse pointer and
// lifted only where a touch has already been resolved as typing.
//
// What can be asserted here is the state machine, not the platform: a headless
// browser has no IME to raise. Every assertion is therefore about the attribute
// Chrome reads when it decides, plus the focus re-entry that re-arms its request
// — which is exactly the pair that has to be right for the phone to behave.

import { animationFrames, glyphRect, settle } from "./browser-touch-helpers.mjs";

const SOURCE = "alpha beta gamma delta epsilon zeta\nsecond line of prose here\n";

export async function runSoftKeyboardGateTests(ContinuityEditorElement, check) {
  return matchMedia("(pointer: coarse)").matches
    ? runCoarsePointerCases(ContinuityEditorElement, check)
    : runFinePointerCase(ContinuityEditorElement, check);
}

/**
 * Desktop is untouched by the gate. Mouse and pen already focus on pointerdown
 * and `inputmode` means nothing to a physical keyboard, so the attribute must
 * never appear on a fine pointer — including on a touchscreen laptop, where it
 * would suppress the platform's on-screen keyboard for no reason.
 */
async function runFinePointerCase(ContinuityEditorElement, check) {
  const { editor, input, dispose } = await mount(ContinuityEditorElement);
  let assertions = 0;
  check(!input.hasAttribute("inputmode"),
    "a fine-pointer editor never gates its input mode"); assertions += 1;
  editor.insertText("x");
  await settle();
  check(!input.hasAttribute("inputmode"),
    "an insert on a fine pointer leaves the input mode alone"); assertions += 1;
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

  const line = projection.children[0];
  const rendered = line.textContent;
  const at = (word, offset) => {
    const rect = glyphRect(line, rendered.indexOf(word) + offset);
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

  dispose();
  return assertions;
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
