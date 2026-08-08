import { hasCoarsePointer } from "./code_affordances.js";
import { toggleCommandRailSettings } from "./command_rail_settings.js";
import {
  railStorageKey,
  reconcileRailConfiguration,
  resolveRailConfiguration,
  saveRailConfiguration,
} from "./command_rail_registry.js";

/**
 * Attach the bottom quick-action rail to the editor frame. The rail shows on
 * touch-primary devices by default (`command-rail="auto"`), can be forced with
 * `command-rail="on|off"`, and hides while the editor is read-only. Buttons
 * cancel `pointerdown` so the semantic textarea keeps focus and the virtual
 * keyboard stays open. The gear button opens a settings panel that toggles
 * and reorders buttons; the arrangement persists in `localStorage` under
 * `continuity-editor.command-rail`, scoped by the `rail-storage-key`
 * attribute when a host runs several distinct rails on one origin.
 *
 * `hooks` carries the four ways a button can act: `execute(command)` for
 * engine commands, `runOperation(name)` for browser-layer operations that
 * have no storage-neutral command (caret motion across wrapped rows),
 * `emitRequest` for host-mediated actions, and `snapshot()` for per-action
 * enablement predicates. `reportError` contains a throwing host callback.
 */
export function attachCommandRail(host, dom, hooks, signal) {
  const rail = document.createElement("div");
  rail.className = "command-rail";
  rail.setAttribute("part", "command-rail");
  rail.setAttribute("role", "toolbar");
  rail.setAttribute("aria-label", "Editor quick actions");
  rail.hidden = true;
  const strip = document.createElement("div");
  strip.className = "command-rail-buttons";
  const settingsButton = document.createElement("button");
  settingsButton.type = "button";
  settingsButton.className = "command-rail-button command-rail-settings-button";
  settingsButton.setAttribute("part", "command-rail-button command-rail-settings-button");
  settingsButton.textContent = "⚙";
  settingsButton.setAttribute("aria-label", "Command rail settings");
  settingsButton.setAttribute("aria-expanded", "false");
  settingsButton.addEventListener("pointerdown", (event) => event.preventDefault());
  rail.append(strip, settingsButton);
  dom.frame.append(rail);

  const registry = hooks.registry;
  let state = resolveRailConfiguration(registry, railStorageKey(host.getAttribute("rail-storage-key")));
  const renderStrip = () => renderRailButtons(strip, state, hooks, signal);
  const applyChange = () => {
    saveRailConfiguration(state);
    renderStrip();
  };
  const closeSettings = () => {
    toggleCommandRailSettings(dom.frame, state, registry, applyChange, false);
    settingsButton.setAttribute("aria-expanded", "false");
  };
  settingsButton.addEventListener("click", () => {
    const isOpen = toggleCommandRailSettings(dom.frame, state, registry, applyChange);
    settingsButton.setAttribute("aria-expanded", String(isOpen));
  }, { signal });

  // A host may register actions long after the editor is ready. Re-resolve
  // against the live arrangement so a late registration lands in its saved
  // slot rather than the rail's tail.
  registry.onChange(() => {
    state = reconcileRailConfiguration(state, registry);
    closeSettings();
    renderStrip();
  });

  const refresh = () => applyRailEnablement(strip, state, hooks);
  const update = () => {
    const storageKey = railStorageKey(host.getAttribute("rail-storage-key"));
    if (storageKey !== state.storageKey) {
      state = resolveRailConfiguration(registry, storageKey);
      renderStrip();
    }
    const isActive = resolveRailActive(host) && !host.readOnly;
    rail.hidden = !isActive;
    dom.frame.classList.toggle("command-rail-active", isActive);
    if (!isActive) closeSettings();
    if (isActive) refresh();
  };
  const pointerQuery = globalThis.matchMedia?.("(pointer: coarse)");
  pointerQuery?.addEventListener?.("change", update, { signal });
  // Enablement predicates read the document, so every commit re-evaluates them.
  host.addEventListener("continuity-change", refresh, { signal });
  renderStrip();
  update();
  return { update, refresh };
}

function resolveRailActive(host) {
  const mode = host.getAttribute("command-rail");
  if (mode === "on") return true;
  if (mode === "off") return false;
  return hasCoarsePointer();
}

function renderRailButtons(strip, state, hooks, signal) {
  const fragment = document.createDocumentFragment();
  for (const entry of state.order) {
    if (state.disabled.has(entry.id)) continue;
    fragment.append(railButton(entry, hooks, signal));
  }
  strip.replaceChildren(fragment);
  applyRailEnablement(strip, state, hooks);
}

function railButton(entry, hooks, signal) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "command-rail-button";
  button.dataset.railAction = entry.id;
  button.setAttribute("part", `command-rail-button command-rail-button-${partToken(entry.id)}`);
  button.setAttribute("aria-label", entry.label);
  button.append(railGlyph(entry, hooks));
  // Keep textarea focus so the virtual keyboard never dismisses mid-action.
  button.addEventListener("pointerdown", (event) => event.preventDefault());
  button.addEventListener("click", () => runRailAction(entry, hooks), { signal });
  return button;
}

function railGlyph(entry, hooks) {
  const glyph = document.createElement("span");
  glyph.className = `command-rail-glyph command-rail-glyph-${partToken(entry.id)}`;
  if (entry.icon) {
    // A host icon is a factory, never markup: building the node host-side keeps
    // untrusted HTML out of the editor's shadow root.
    try {
      const node = entry.icon();
      if (node instanceof Element) {
        glyph.append(node);
        return glyph;
      }
    } catch (error) {
      hooks.reportError(error);
    }
  }
  glyph.textContent = entry.glyph;
  return glyph;
}

function applyRailEnablement(strip, state, hooks) {
  const predicated = state.order.filter((entry) => entry.isEnabled && !state.disabled.has(entry.id));
  if (predicated.length === 0) return;
  const snapshot = hooks.snapshot();
  for (const entry of predicated) {
    const button = strip.querySelector(`[data-rail-action="${cssEscape(entry.id)}"]`);
    if (!button) continue;
    let isEnabled = true;
    try {
      isEnabled = snapshot ? entry.isEnabled(snapshot) !== false : true;
    } catch (error) {
      hooks.reportError(error);
    }
    button.disabled = !isEnabled;
  }
}

function runRailAction(entry, hooks) {
  if (entry.operation) {
    hooks.runOperation(entry.operation);
    return;
  }
  if (entry.command) {
    hooks.execute(entry.command);
    return;
  }
  if (entry.run) {
    try {
      entry.run(hooks.editorElement);
    } catch (error) {
      hooks.reportError(error);
    }
    return;
  }
  // Neither a command nor a callback: a declarative host answers the request.
  hooks.emitRequest("railAction", { actionId: entry.id });
}

/** Reduce an action id to a CSS-addressable `::part()` / class token. */
function partToken(id) {
  return id.replace(/[^a-z0-9-]+/giu, "-");
}

function cssEscape(value) {
  return globalThis.CSS?.escape ? globalThis.CSS.escape(value) : value.replace(/:/gu, "\\:");
}
