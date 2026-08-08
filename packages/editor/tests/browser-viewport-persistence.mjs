// Viewport persistence across host hide/show. Tab-style hosts often keep the
// editor mounted and toggle display:none; the browser discards the hidden
// textarea's layout and zeroes its scroll, so on re-show the note snapped to
// the top. The resize observer must ignore the zero-width hide and reassert
// the tracked viewport when layout returns.
export async function runViewportPersistenceTests(ContinuityEditorElement, check) {
  let assertions = 0;
  const mount = document.querySelector("#mount");
  const editor = new ContinuityEditorElement();
  editor.setAttribute("aria-label", "Viewport persistence document");
  editor.style.height = "160px";
  editor.value = Array.from({ length: 200 }, (_, index) => `line ${index}`).join("\n");
  mount.append(editor);
  await editor.ready;
  const input = editor.shadowRoot.querySelector("textarea");
  await animationFrames(2);

  input.scrollTop = 900;
  input.dispatchEvent(new Event("scroll", { bubbles: true }));
  const scrolledTo = input.scrollTop;
  check(scrolledTo > 0, "the long document accepts a mid-document viewport"); assertions += 1;

  editor.style.display = "none";
  await animationFrames(3);
  editor.style.display = "";
  await animationFrames(3);
  check(Math.abs(input.scrollTop - scrolledTo) <= 1,
    `the viewport survives a display:none hide/show cycle (scrollTop=${input.scrollTop}, expected=${scrolledTo})`);
  assertions += 1;

  const state = editor.getScrollState();
  check(Math.abs(state.top - scrolledTo) <= 1,
    "getScrollState reports the restored viewport for host-side tab state"); assertions += 1;

  editor.destroy();
  editor.remove();
  return assertions;
}

function animationFrames(count) {
  return new Promise((resolve) => {
    const next = () => count-- <= 0 ? resolve() : requestAnimationFrame(next);
    requestAnimationFrame(next);
  });
}
