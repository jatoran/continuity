/** Apply host-owned presentation and input policy to one editor element. */
export function applyEditorConfiguration(element, configuration) {
  element.readOnly = Boolean(configuration.readOnly);
  element.spellcheck = configuration.spellcheck ?? true;
  element.shortcutPolicy = configuration.shortcutPolicy ?? "browser-safe";
  element.shortcutBindings = configuration.shortcutBindings ?? {};
  // Rail actions are objects with callbacks, so they travel as a property
  // rather than an attribute; assigning replaces the host set wholesale.
  if (configuration.railActions) element.railActions = configuration.railActions;
  applyOptionalAttribute(element, "syntax", configuration.syntax);
  applyOptionalAttribute(element, "theme", configuration.theme);
  applyOptionalAttribute(element, "tab-behavior", configuration.tabBehavior);
  applyOptionalAttribute(element, "indent-guides", configuration.indentGuides);
}

/** Seed an editor before its asynchronous WASM initialization resumes. */
export function initializeControlledElement(element, value, revision) {
  element.value = String(value);
  element.initialRevision = revision;
}

/** Reconcile a changed controlled snapshot without overwriting newer typing. */
export async function synchronizeControlledSnapshot(element, value, revision) {
  await element.ready;
  if (element.composing) {
    return null;
  }
  const snapshot = element.snapshot();
  const normalizedValue = String(value);
  if (snapshot.text === normalizedValue && snapshot.revision === revision) {
    return null;
  }
  return element.replaceValue(normalizedValue, revision);
}

function applyOptionalAttribute(element, name, value) {
  if (value === undefined || value === null || value === "") {
    element.removeAttribute(name);
  } else {
    element.setAttribute(name, String(value));
  }
}
