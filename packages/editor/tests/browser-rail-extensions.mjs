// Command-rail extension contract: built-in line movement and visible-row
// caret motion, host-registered actions (engine command, host callback, host
// request), enablement predicates, per-button parts, and the arrangement
// persistence that has to survive a host registering after the editor is ready.
const STORAGE_KEY = "continuity-editor.command-rail";

export async function runRailExtensionTests(ContinuityEditorElement, check) {
  let assertions = 0;
  const mount = document.querySelector("#mount");
  localStorage.removeItem(STORAGE_KEY);

  const editor = await mountRailEditor(ContinuityEditorElement, mount, {
    value: "alpha\nbravo\ncharlie\n",
    label: "Rail extension document",
  });
  const shadow = editor.shadowRoot;
  const input = shadow.querySelector("textarea");
  const rail = shadow.querySelector(".command-rail");

  // Line movement: the engine moves whole source lines covering the selection,
  // so a selection spanning two lines carries both, and soft wrap is irrelevant.
  selectRange(input, 0, 0);
  rail.querySelector('[data-rail-action="move-line-down"]').click();
  check(editor.snapshot().text.startsWith("bravo\nalpha\n"),
    "the move-line-down rail action swaps the caret line with the one below"); assertions += 1;
  rail.querySelector('[data-rail-action="move-line-up"]').click();
  check(editor.snapshot().text.startsWith("alpha\nbravo\n"),
    "the move-line-up rail action moves it back"); assertions += 1;

  selectRange(input, 0, 8);
  rail.querySelector('[data-rail-action="move-line-down"]').click();
  check(editor.snapshot().text === "charlie\nalpha\nbravo\n",
    "a multi-line selection moves as one block"); assertions += 1;
  check(shadow.activeElement === input,
    "line movement keeps the semantic textarea focused"); assertions += 1;
  rail.querySelector('[data-rail-action="undo"]').click();
  check(editor.snapshot().text === "alpha\nbravo\ncharlie\n",
    "one undo reverts a whole block move"); assertions += 1;
  editor.destroy();
  editor.remove();

  assertions += await runVisibleRowCaretTests(ContinuityEditorElement, mount, check);
  assertions += await runHostActionTests(ContinuityEditorElement, mount, check);
  assertions += await runPersistenceTests(ContinuityEditorElement, mount, check);
  localStorage.removeItem(STORAGE_KEY);
  return assertions;
}

// A wrapped source line occupies several visible rows. The caret actions walk
// those rows like the platform arrow keys, so the source line does not change.
async function runVisibleRowCaretTests(ContinuityEditorElement, mount, check) {
  let assertions = 0;
  const editor = await mountRailEditor(ContinuityEditorElement, mount, {
    value: `${"wrap ".repeat(60)}\nnext line\n`,
    label: "Wrapped caret document",
    width: "240px",
  });
  const shadow = editor.shadowRoot;
  const input = shadow.querySelector("textarea");
  const rail = shadow.querySelector(".command-rail");
  const rowHeight = parseFloat(getComputedStyle(input).lineHeight) || 24;
  check(input.scrollHeight > rowHeight * 2,
    "the fixture line wraps across several visible rows"); assertions += 1;

  const startOffset = 150;
  selectRange(input, startOffset, startOffset);
  const startTop = caretTop(input, startOffset);
  rail.querySelector('[data-rail-action="caret-up"]').click();
  const afterUp = input.selectionStart;
  check(afterUp < startOffset && afterUp > 0,
    "caret-up moves backwards inside the wrapped line"); assertions += 1;
  check(editor.snapshot().selections[0].head.line === 0,
    "caret-up stays on the same source line, moving one visible row"); assertions += 1;
  check(Math.abs((startTop - caretTop(input, afterUp)) - rowHeight) < rowHeight * 0.5,
    "caret-up lands exactly one visible row higher"); assertions += 1;
  check(Math.abs(caretLeft(input, afterUp) - caretLeft(input, startOffset)) < 12,
    "caret-up keeps the horizontal position across the wrapped row"); assertions += 1;
  check(shadow.activeElement === input,
    "caret motion keeps the semantic textarea focused"); assertions += 1;

  rail.querySelector('[data-rail-action="caret-down"]').click();
  check(Math.abs(input.selectionStart - startOffset) <= 2,
    "caret-down returns to the row it came from"); assertions += 1;
  check(editor.snapshot().text.startsWith("wrap "),
    "caret motion never edits the document"); assertions += 1;

  // The top row has no row above it inside this line; the caret must not jump.
  selectRange(input, 2, 2);
  rail.querySelector('[data-rail-action="caret-up"]').click();
  check(input.selectionStart === 2 || input.selectionStart === 0,
    "caret-up on the first visible row does not leave the document"); assertions += 1;
  editor.destroy();
  editor.remove();
  return assertions;
}

async function runHostActionTests(ContinuityEditorElement, mount, check) {
  let assertions = 0;
  const editor = await mountRailEditor(ContinuityEditorElement, mount, {
    value: "alpha\n",
    label: "Host rail action document",
  });
  const shadow = editor.shadowRoot;
  const input = shadow.querySelector("textarea");
  const rail = shadow.querySelector(".command-rail");

  const runs = [];
  const requests = [];
  editor.addEventListener("continuity-request", (event) => {
    if (event.detail.kind === "railAction") requests.push(event.detail.actionId);
  });
  const disposeCommand = editor.registerRailAction({
    id: "acme:uppercase", label: "Uppercase", glyph: "AA", command: "editor.change_case_upper",
  });
  editor.registerRailAction({
    id: "acme:callback",
    label: "Host callback",
    icon: () => {
      const mark = document.createElementNS("http://www.w3.org/2000/svg", "svg");
      mark.dataset.testIcon = "true";
      return mark;
    },
    run: (element) => runs.push(element.snapshot().text),
  });
  editor.registerRailAction({ id: "acme:mediated", label: "Host mediated" });
  editor.registerRailAction({
    id: "acme:selection-only",
    label: "Selection only",
    command: "markdown.toggle_bold",
    isEnabled: (snapshot) => snapshot.selections.some(
      (selection) => selection.anchor.byteInLine !== selection.head.byteInLine,
    ),
  });

  check(rail.querySelectorAll('[data-rail-action^="acme:"]').length === 4,
    "registered host actions render on the rail"); assertions += 1;
  check(editor.railActions.length === 4 && editor.railActions[0].id === "acme:uppercase",
    "railActions reports the registered host descriptors"); assertions += 1;
  const commandButton = rail.querySelector('[data-rail-action="acme:uppercase"]');
  check(commandButton.getAttribute("part") === "command-rail-button command-rail-button-acme-uppercase",
    "each rail button exposes an addressable part for host styling"); assertions += 1;
  check(Boolean(rail.querySelector('[data-rail-action="acme:callback"] svg[data-test-icon]')),
    "a host icon factory renders its own element inside the button"); assertions += 1;

  selectRange(input, 0, 5);
  commandButton.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, cancelable: true }));
  commandButton.click();
  check(editor.snapshot().text.startsWith("ALPHA"),
    "a host action naming an engine command runs it through the shared resolver"); assertions += 1;
  check(shadow.activeElement === input,
    "host rail actions keep the semantic textarea focused"); assertions += 1;

  rail.querySelector('[data-rail-action="acme:callback"]').click();
  check(runs.length === 1 && runs[0].startsWith("ALPHA"),
    "a host callback action receives the editor element"); assertions += 1;
  rail.querySelector('[data-rail-action="acme:mediated"]').click();
  check(requests.length === 1 && requests[0] === "acme:mediated",
    "an action with neither command nor callback raises a railAction request"); assertions += 1;

  // Enablement predicates re-evaluate on selection and on every commit.
  const predicated = rail.querySelector('[data-rail-action="acme:selection-only"]');
  selectRange(input, 0, 5);
  check(predicated.disabled === false,
    "a range selection leaves the predicated action enabled"); assertions += 1;
  selectRange(input, 0, 0);
  check(predicated.disabled === true,
    "collapsing the selection disables the predicated action"); assertions += 1;

  const errors = [];
  editor.addEventListener("continuity-error", (event) => errors.push(event.detail.error));
  editor.registerRailAction({
    id: "acme:throws", label: "Throws", run: () => { throw new Error("host failure"); },
  });
  rail.querySelector('[data-rail-action="acme:throws"]').click();
  check(errors.length === 1 && String(errors[0].message).includes("host failure"),
    "a throwing host action surfaces as continuity-error instead of breaking the rail"); assertions += 1;
  check(Boolean(rail.querySelector('[data-rail-action="acme:uppercase"]')),
    "the rail survives a throwing host action"); assertions += 1;

  disposeCommand();
  check(!rail.querySelector('[data-rail-action="acme:uppercase"]'),
    "disposing a registration removes its button"); assertions += 1;

  for (const [descriptor, reason] of [
    [{ id: "uppercase", label: "Bare id" }, "an un-namespaced id is rejected"],
    [{ id: "undo:x", label: "Fine" }, "a namespaced id is accepted"],
    [{ id: "bold", label: "Built-in" }, "a built-in id is rejected"],
    [{ id: "acme:callback", label: "Duplicate" }, "a duplicate registration is rejected"],
    [{ id: "acme:nolabel" }, "a missing label is rejected"],
  ]) {
    let threw = false;
    let dispose = null;
    try {
      dispose = editor.registerRailAction(descriptor);
    } catch {
      threw = true;
    }
    dispose?.();
    check(reason.includes("accepted") ? !threw : threw, `rail registration: ${reason}`);
    assertions += 1;
  }

  editor.railActions = [{ id: "acme:only", label: "Only one", command: "editor.undo" }];
  check(rail.querySelectorAll('[data-rail-action^="acme:"]').length === 1,
    "assigning railActions replaces the whole host set"); assertions += 1;
  check(Boolean(rail.querySelector('[data-rail-action="bold"]')),
    "replacing host actions leaves the built-in catalog intact"); assertions += 1;
  editor.destroy();
  editor.remove();
  return assertions;
}

async function runPersistenceTests(ContinuityEditorElement, mount, check) {
  let assertions = 0;
  // An arrangement saved while a host action was registered must survive a
  // reload where the host registers late: pruning the id would silently reset
  // the user's rail on every load.
  localStorage.setItem(STORAGE_KEY, JSON.stringify({
    order: ["acme:late", "undo", "redo", "caret-up", "caret-down"],
    disabled: ["redo", "acme:absent"],
  }));
  const editor = await mountRailEditor(ContinuityEditorElement, mount, {
    value: "alpha\n", label: "Rail persistence document",
  });
  const rail = editor.shadowRoot.querySelector(".command-rail");
  check(rail.querySelector("[data-rail-action]").dataset.railAction === "undo",
    "an unregistered stored id does not render a button"); assertions += 1;
  editor.registerRailAction({ id: "acme:late", label: "Late", command: "editor.undo" });
  const order = [...rail.querySelectorAll("[data-rail-action]")].map((b) => b.dataset.railAction);
  check(order[0] === "acme:late",
    "a late registration returns to its persisted slot"); assertions += 1;
  check(!order.includes("redo"),
    "a disabled built-in stays disabled across the reconciliation"); assertions += 1;

  // Force a save (any settings change) and confirm the round trip keeps the
  // ids belonging to actions this editor never saw registered.
  rail.querySelector(".command-rail-settings-button").click();
  editor.shadowRoot
    .querySelector('[data-rail-setting="bold"] .command-rail-settings-toggle').click();
  const stored = JSON.parse(localStorage.getItem(STORAGE_KEY));
  check(stored.order[0] === "acme:late" && stored.disabled.includes("acme:absent"),
    "saving keeps ids whose actions are not registered, at their slot"); assertions += 1;
  check(stored.disabled.includes("bold") && stored.disabled.includes("redo"),
    "saving records the live enable/disable state alongside the retained ids"); assertions += 1;

  // Two rails on one origin must not share one arrangement.
  const scoped = await mountRailEditor(ContinuityEditorElement, mount, {
    value: "alpha\n", label: "Scoped rail document", storageKey: "notes",
  });
  const scopedRail = scoped.shadowRoot.querySelector(".command-rail");
  check(scopedRail.querySelector("[data-rail-action]").dataset.railAction === "undo"
    && Boolean(scopedRail.querySelector('[data-rail-action="redo"]')),
  "rail-storage-key scopes the arrangement away from the default key"); assertions += 1;
  scopedRail.querySelector(".command-rail-settings-button").click();
  scoped.shadowRoot
    .querySelector('[data-rail-setting="redo"] .command-rail-settings-toggle').click();
  check(JSON.parse(localStorage.getItem(`${STORAGE_KEY}:notes`)).disabled.includes("redo")
    && !JSON.parse(localStorage.getItem(STORAGE_KEY)).disabled.includes("acme:late"),
  "a scoped rail persists under its own key"); assertions += 1;
  localStorage.removeItem(`${STORAGE_KEY}:notes`);
  scoped.destroy();
  scoped.remove();
  editor.destroy();
  editor.remove();
  return assertions;
}

async function mountRailEditor(ContinuityEditorElement, mount, options) {
  const editor = new ContinuityEditorElement();
  editor.setAttribute("command-rail", "on");
  editor.setAttribute("aria-label", options.label);
  if (options.storageKey) editor.setAttribute("rail-storage-key", options.storageKey);
  if (options.width) {
    editor.style.width = options.width;
    editor.style.height = "260px";
  }
  editor.value = options.value;
  mount.append(editor);
  await editor.ready;
  editor.focus();
  return editor;
}

function selectRange(input, start, end) {
  input.focus();
  input.setSelectionRange(start, end);
  input.dispatchEvent(new Event("select", { bubbles: true }));
}

function caretRect(input, offset) {
  const mirror = document.createElement("div");
  const style = getComputedStyle(input);
  mirror.style.cssText = "position:fixed;left:-10000px;top:0;visibility:hidden;white-space:pre-wrap;overflow-wrap:anywhere;box-sizing:border-box";
  for (const property of ["font", "letterSpacing", "lineHeight", "padding", "border", "tabSize", "textIndent", "wordSpacing"]) {
    mirror.style[property] = style[property];
  }
  mirror.style.width = `${input.clientWidth}px`;
  const text = document.createTextNode(`${input.value}​`);
  mirror.append(text);
  document.body.append(mirror);
  const range = document.createRange();
  range.setStart(text, offset);
  range.collapse(true);
  const rect = range.getBoundingClientRect();
  const measured = { top: rect.top, left: rect.left };
  mirror.remove();
  return measured;
}

function caretTop(input, offset) {
  return caretRect(input, offset).top;
}

function caretLeft(input, offset) {
  return caretRect(input, offset).left;
}
