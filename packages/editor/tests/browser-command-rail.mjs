// Bottom quick-action rail (touch-primary chrome) and touch code-copy lane.
// The rail must execute shared engine commands without stealing textarea
// focus, its arrangement must be editable and persistent, and on touch
// surfaces the block code-copy control must be visible without hover.
export async function runCommandRailTests(ContinuityEditorElement, check) {
  let assertions = 0;
  const mount = document.querySelector("#mount");
  localStorage.removeItem("continuity-editor.command-rail");

  const editor = new ContinuityEditorElement();
  editor.setAttribute("command-rail", "on");
  editor.setAttribute("aria-label", "Command rail document");
  editor.value = "alpha\nbravo\n";
  mount.append(editor);
  await editor.ready;
  const shadow = editor.shadowRoot;
  const input = shadow.querySelector("textarea");
  const rail = shadow.querySelector(".command-rail");
  check(Boolean(rail) && !rail.hidden
    && shadow.querySelector(".frame").classList.contains("command-rail-active")
    && getComputedStyle(input).bottom !== "0px",
  "command-rail=on shows the rail and reserves textarea viewport height"); assertions += 1;

  // Bullet rail action: content and caret both advance by the two-character
  // marker so the cursor stays over the same character it started on.
  input.focus();
  input.setSelectionRange(2, 2);
  input.dispatchEvent(new Event("select", { bubbles: true }));
  const bullet = rail.querySelector('[data-rail-action="bullet"]');
  bullet.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, cancelable: true }));
  bullet.click();
  check(editor.snapshot().text.startsWith("- alpha"),
    "a rail action executes the shared engine command"); assertions += 1;
  check(shadow.activeElement === input,
    "rail actions keep the semantic textarea focused"); assertions += 1;
  check(input.selectionStart === 4 && input.selectionEnd === 4,
    "the bullet rail action advances the textarea caret onto the same content character"); assertions += 1;
  check(editor.snapshot().selections[0].head.byteInLine === 4,
    "the bullet rail action advances the engine caret content-relative"); assertions += 1;
  rail.querySelector('[data-rail-action="undo"]').click();
  check(editor.snapshot().text.startsWith("alpha"),
    "the rail undo action reverts the previous rail action"); assertions += 1;
  check(editor.snapshot().selections[0].head.byteInLine === 2,
    "undoing the bullet rail action returns the caret to the original content position"); assertions += 1;

  // Checkmark rail action creates a full Markdown task (`- [ ] `), not a bare
  // inline `[ ] ` checkbox prefix; focus stays on the semantic textarea.
  input.focus();
  input.setSelectionRange(0, 0);
  input.dispatchEvent(new Event("select", { bubbles: true }));
  const checkmark = rail.querySelector('[data-rail-action="checkbox"]');
  checkmark.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, cancelable: true }));
  checkmark.click();
  check(editor.snapshot().text.startsWith("- [ ] alpha"),
    "the checkmark rail action creates a task bullet (`- [ ] `), not a bare `[ ] ` prefix"); assertions += 1;
  check(shadow.activeElement === input,
    "the checkmark rail action keeps the semantic textarea focused"); assertions += 1;
  rail.querySelector('[data-rail-action="undo"]').click();
  check(editor.snapshot().text.startsWith("alpha"),
    "undoing the checkmark rail action reverts to plain text"); assertions += 1;

  const gear = rail.querySelector(".command-rail-settings-button");
  gear.click();
  const panel = shadow.querySelector(".command-rail-settings");
  check(Boolean(panel) && gear.getAttribute("aria-expanded") === "true",
    "the gear opens the command rail settings panel"); assertions += 1;
  panel.querySelector('[data-rail-setting="bullet"] .command-rail-settings-toggle').click();
  check(!rail.querySelector('[data-rail-action="bullet"]'),
    "disabling a quick action removes it from the rail"); assertions += 1;
  panel.querySelector('[data-rail-setting="undo"]')
    .querySelectorAll(".command-rail-settings-move")[1].click();
  const order = [...rail.querySelectorAll("[data-rail-action]")].map((b) => b.dataset.railAction);
  check(order[0] === "redo" && order[1] === "undo",
    "reordering in settings rearranges the rail"); assertions += 1;
  const stored = JSON.parse(localStorage.getItem("continuity-editor.command-rail"));
  check(stored.disabled.includes("bullet") && stored.order[0] === "redo",
    "the rail arrangement persists to localStorage"); assertions += 1;
  panel.querySelector(".command-rail-settings-action").click();
  check(Boolean(rail.querySelector('[data-rail-action="bullet"]')),
    "reset restores the default arrangement"); assertions += 1;

  editor.readOnly = true;
  check(rail.hidden === true, "the rail hides while the editor is read-only"); assertions += 1;
  editor.readOnly = false;
  editor.setAttribute("command-rail", "off");
  check(rail.hidden === true, "command-rail=off hides the rail"); assertions += 1;
  localStorage.removeItem("continuity-editor.command-rail");
  editor.destroy();
  editor.remove();

  // Host-root-font-size independence: rail chrome is px-sized, so a dense
  // embedding host (e.g. :root { font-size: 11px }) cannot shrink touch
  // targets below the 44px guideline, and hosts retune via custom properties.
  const rootFontSizeBefore = document.documentElement.style.fontSize;
  document.documentElement.style.fontSize = "11px";
  try {
    const denseEditor = new ContinuityEditorElement();
    denseEditor.setAttribute("command-rail", "on");
    denseEditor.setAttribute("aria-label", "Dense host document");
    denseEditor.value = "alpha\n";
    mount.append(denseEditor);
    await denseEditor.ready;
    const denseShadow = denseEditor.shadowRoot;
    const denseRail = denseShadow.querySelector(".command-rail");
    const denseInput = denseShadow.querySelector("textarea");
    check(denseRail.querySelector("[data-rail-action]").getBoundingClientRect().height >= 44,
      "rail buttons keep a >=44px touch height under an 11px host root font-size"); assertions += 1;
    check(Math.abs(parseFloat(getComputedStyle(denseInput).bottom)
      - denseRail.getBoundingClientRect().height) < 1,
    "the textarea bottom inset equals the actual rail height"); assertions += 1;
    denseRail.querySelector(".command-rail-settings-button").click();
    check(denseShadow.querySelector(".command-rail-settings-toggle")
      .getBoundingClientRect().height >= 44,
    "settings controls keep a >=44px touch height under a dense host"); assertions += 1;
    denseEditor.style.setProperty("--continuity-rail-height", "64px");
    check(Math.abs(denseRail.getBoundingClientRect().height - 64) < 1
      && Math.abs(parseFloat(getComputedStyle(denseInput).bottom) - 64) < 1,
    "--continuity-rail-height retunes the rail and content inset together"); assertions += 1;
    denseEditor.destroy();
    denseEditor.remove();
  } finally {
    document.documentElement.style.fontSize = rootFontSizeBefore;
  }

  // Touch surface: pointer-coarse media makes the auto rail show and the
  // fenced code-copy control permanently visible (touch has no hover, and a
  // tap-revealed control used to die on the next projection rebuild).
  const originalMatchMedia = window.matchMedia;
  window.matchMedia = (query) => (query === "(pointer: coarse)"
    ? { matches: true, addEventListener() {}, removeEventListener() {} }
    : originalMatchMedia.call(window, query));
  try {
    const touchEditor = new ContinuityEditorElement();
    touchEditor.setAttribute("aria-label", "Touch copy document");
    touchEditor.value = "intro\n\n```js\nconst value = 1;\n```\n";
    mount.append(touchEditor);
    await touchEditor.ready;
    const copyControl = touchEditor.shadowRoot.querySelector(".code-copy-block");
    check(copyControl?.classList.contains("visible")
      && copyControl.dataset.alwaysVisible === "true",
    "the block code-copy control is visible without hover on touch surfaces"); assertions += 1;
    check(!touchEditor.shadowRoot.querySelector(".command-rail").hidden,
      "command-rail auto mode shows the rail on touch surfaces"); assertions += 1;
    touchEditor.destroy();
    touchEditor.remove();
  } finally {
    window.matchMedia = originalMatchMedia;
  }
  return assertions;
}
