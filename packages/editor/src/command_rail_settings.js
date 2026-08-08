import { resetRailConfiguration } from "./command_rail_registry.js";

/**
 * Toggle the command-rail settings panel inside the editor frame. Each row
 * enables/disables one quick action and moves it earlier or later in the rail.
 * Host-registered actions appear alongside the built-ins and are arranged the
 * same way. Mutations edit the shared rail state and report through
 * `applyChange`, which persists and re-renders the rail. Returns whether the
 * panel is now open. Pass `forceOpen: false` to close unconditionally (rail
 * deactivation).
 */
export function toggleCommandRailSettings(frame, state, registry, applyChange, forceOpen) {
  const existing = frame.querySelector(".command-rail-settings");
  const shouldOpen = forceOpen ?? !existing;
  existing?.remove();
  if (!shouldOpen) {
    return false;
  }
  const panel = document.createElement("div");
  panel.className = "command-rail-settings";
  panel.setAttribute("part", "command-rail-settings");
  panel.setAttribute("role", "group");
  panel.setAttribute("aria-label", "Command rail settings");
  panel.addEventListener("pointerdown", (event) => event.preventDefault());
  renderSettingsRows(panel, state, registry, applyChange);
  frame.append(panel);
  return true;
}

function renderSettingsRows(panel, state, registry, applyChange) {
  const fragment = document.createDocumentFragment();
  state.order.forEach((entry, index) => {
    fragment.append(settingsRow(panel, state, registry, applyChange, entry, index));
  });
  const footer = document.createElement("div");
  footer.className = "command-rail-settings-footer";
  const reset = document.createElement("button");
  reset.type = "button";
  reset.className = "command-rail-settings-action";
  reset.textContent = "Reset";
  reset.setAttribute("aria-label", "Reset command rail to defaults");
  reset.addEventListener("click", () => {
    resetRailConfiguration(state, registry);
    applyChange();
    renderSettingsRows(panel, state, registry, applyChange);
  });
  footer.append(reset);
  fragment.append(footer);
  panel.replaceChildren(fragment);
}

function settingsRow(panel, state, registry, applyChange, entry, index) {
  const row = document.createElement("div");
  row.className = "command-rail-settings-row";
  row.dataset.railSetting = entry.id;
  const isEnabled = !state.disabled.has(entry.id);
  const toggle = document.createElement("button");
  toggle.type = "button";
  toggle.className = "command-rail-settings-toggle";
  toggle.setAttribute("aria-pressed", String(isEnabled));
  toggle.setAttribute("aria-label", `${isEnabled ? "Disable" : "Enable"} ${entry.label}`);
  toggle.textContent = isEnabled ? "✓" : "";
  toggle.addEventListener("click", () => {
    if (state.disabled.has(entry.id)) {
      state.disabled.delete(entry.id);
    } else {
      state.disabled.add(entry.id);
    }
    applyChange();
    renderSettingsRows(panel, state, registry, applyChange);
  });
  const name = document.createElement("span");
  name.className = "command-rail-settings-name";
  // A host icon action has no text glyph; its label carries the whole row.
  name.textContent = entry.glyph ? `${entry.glyph} ${entry.label}` : entry.label;
  row.append(toggle, name);
  row.append(
    moveButton(panel, state, registry, applyChange, entry, index, -1),
    moveButton(panel, state, registry, applyChange, entry, index, 1),
  );
  return row;
}

function moveButton(panel, state, registry, applyChange, entry, index, delta) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "command-rail-settings-move";
  button.textContent = delta < 0 ? "▲" : "▼";
  button.setAttribute(
    "aria-label",
    `Move ${entry.label} ${delta < 0 ? "earlier" : "later"}`,
  );
  const target = index + delta;
  button.disabled = target < 0 || target >= state.order.length;
  button.addEventListener("click", () => {
    state.order.splice(index, 1);
    state.order.splice(target, 0, entry);
    applyChange();
    renderSettingsRows(panel, state, registry, applyChange);
  });
  return button;
}
