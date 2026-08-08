import {
  ContinuityEditorElement,
  initialize,
} from "./node_modules/@continuity-editor/editor/index.js";

const host = window.continuityDesktop;
if (!host) {
  throw new Error("Continuity desktop preload bridge is unavailable");
}

const initial = await host.load();
await initialize({ wasm: await host.wasm() });

const editorHost = document.querySelector("#editor-host");
const status = document.querySelector("#status");
const documentName = document.querySelector("#document-name");
const banner = document.querySelector("#banner");
const bannerText = document.querySelector("#banner-text");
const bannerActions = document.querySelector("#banner-actions");
const editor = new ContinuityEditorElement();

let settings = initial.settings;
let currentAssociation = initial.associatedFile;
let currentSnapshot = {
  text: initial.text,
  revision: initial.revision,
  selections: initial.selections,
  isReadOnly: false,
};
let latestPersistence = Promise.resolve();
let hasPersistenceFailure = false;

editor.value = initial.text;
editor.initialRevision = initial.revision;
editor.shortcutPolicy = "editor-first";
editor.setAttribute("aria-label", "Continuity markdown document");
editorHost.append(editor);
applyTheme(settings.theme);
await editor.ready;
currentSnapshot = editor.snapshot();
updateDocumentName();

if (initial.recovery.recovered) {
  showBanner(`Recovered revision ${initial.revision}; ${initial.recovery.discardedCandidates} interrupted write was ignored.`);
} else {
  setStatus(`Ready at revision ${initial.revision}.`);
}

editor.addEventListener("continuity-change", (event) => {
  currentSnapshot = event.detail.snapshot;
  latestPersistence = latestPersistence.then(async () => {
    const acknowledgement = await host.persist(event.detail);
    setStatus(`Saved revision ${acknowledgement.revision}; durable sequence ${acknowledgement.durableSequence}.`);
    return acknowledgement;
  }).catch((error) => {
    hasPersistenceFailure = true;
    editor.readOnly = true;
    showBanner(`Persistence failed; editing is paused to protect revision order. ${error}`, [
      action("Copy document", () => host.copyText(editor.snapshot().text)),
    ]);
    throw error;
  });
});

editor.addEventListener("continuity-request", (event) => {
  const detail = event.detail;
  if (detail.kind === "openLink") {
    void host.openLink(detail.href).catch(showHostError);
  } else if (detail.kind === "copyText") {
    void host.copyText(detail.text).catch(showHostError);
  } else if (detail.kind === "contextMenu") {
    showBanner("The preview shell leaves the editor context menu to the operating-system menu bar.");
  } else if (detail.kind === "filesDropped") {
    showBanner("Use File → Open to import a dropped Markdown file safely.");
  }
});

editor.addEventListener("focusin", () => host.setEditorFocused(true));
editor.addEventListener("focusout", () => queueMicrotask(() => {
  host.setEditorFocused(editor.matches(":focus-within"));
}));

host.onEditorCommand((command) => {
  try {
    editor.executeCommand(command);
  } catch (error) {
    showHostError(error);
  }
});
host.onMenuCommand((message) => void handleMenuCommand(message).catch(showHostError));
host.onExternalChange((change) => handleExternalChange(change));
host.onUpdateStatus((update) => handleUpdateStatus(update));
host.onBeforeClose(async () => {
  try {
    await latestPersistence;
  } catch {
    // A failure already froze editing and surfaced a recovery action.
  }
  host.readyToClose();
});

document.querySelector("#new").addEventListener("click", () => void newDocument().catch(showHostError));
document.querySelector("#open").addEventListener("click", () => void openDocument().catch(showHostError));
document.querySelector("#save").addEventListener("click", () => void exportDocument(false).catch(showHostError));

host.rendererReady();

if (host.closeProbe) {
  runCloseProbe();
} else if (host.smokeRun > 0) {
  await runSmoke(host.smokeRun);
}

async function handleMenuCommand(message) {
  const command = typeof message === "string" ? message : message.command;
  if (command === "new") {
    await newDocument();
  } else if (command === "open") {
    await openDocument();
  } else if (command === "save") {
    await exportDocument(false);
  } else if (command === "save-as") {
    await exportDocument(true);
  } else if (command === "open-path") {
    await replaceWithOpened(message.opened);
  } else if (command === "check-updates") {
    await host.checkForUpdates();
  } else if (command === "open-releases") {
    await host.openLink("https://github.com/continuity-editor/continuity/releases");
  } else if (command === "host-error") {
    showBanner(message.message);
  } else if (command?.startsWith("theme:")) {
    settings = await host.saveSettings({ ...settings, theme: command.slice("theme:".length) });
    applyTheme(settings.theme);
  }
}

async function newDocument() {
  if (hasPersistenceFailure) {
    return;
  }
  await latestPersistence;
  await host.newDocument(editor.snapshot());
  currentAssociation = null;
  editor.replaceValue("", editor.snapshot().revision);
  updateDocumentName();
  setStatus("New host-managed document.");
}

async function openDocument() {
  if (hasPersistenceFailure) {
    return;
  }
  await latestPersistence;
  const opened = await host.openDocument();
  if (opened) {
    await replaceWithOpened(opened);
  }
}

async function replaceWithOpened(opened) {
  currentAssociation = opened.association;
  const change = editor.replaceValue(opened.text, editor.snapshot().revision);
  if (!change) {
    await host.persistMetadata(editor.snapshot());
    setStatus("The selected file already matches the current document.");
  }
  updateDocumentName();
}

async function exportDocument(saveAs) {
  if (hasPersistenceFailure) {
    return;
  }
  await latestPersistence;
  const result = await host.exportDocument(editor.snapshot(), saveAs);
  if (result) {
    currentAssociation = result.association;
    updateDocumentName();
    setStatus(`Exported ${result.association.name} at revision ${result.revision}.`);
  }
}

function handleExternalChange(change) {
  if (change.error) {
    showBanner(`Associated file is unavailable: ${change.error}`);
    return;
  }
  showBanner(`${change.association.name} changed outside Continuity.`, [
    action("Reload", async () => {
      await latestPersistence;
      const accepted = await host.acceptExternal("reload", editor.snapshot());
      currentAssociation = accepted.association;
      const change = editor.replaceValue(accepted.text, editor.snapshot().revision);
      if (!change) {
        await host.persistMetadata(editor.snapshot());
      }
      hideBanner();
    }),
    action("Keep editor text", async () => {
      await latestPersistence;
      const accepted = await host.acceptExternal("keep", editor.snapshot());
      currentAssociation = accepted.association;
      hideBanner();
      setStatus("Kept editor text; the next export will replace the external file.");
    }),
  ]);
}

function handleUpdateStatus(update) {
  if (update.state === "ready") {
    showBanner(`Continuity Web ${update.name ?? "update"} is ready.`, [
      action("Restart and install", () => host.installUpdate()),
      action("Later", hideBanner),
    ]);
  } else if (update.state === "error") {
    showBanner(`Update check failed: ${update.message}`);
  } else if (update.state === "current") {
    setStatus("Continuity Web is up to date.");
  } else if (update.state === "checking") {
    setStatus("Checking for updates…");
  }
}

function applyTheme(theme) {
  const resolved = theme === "system"
    ? (matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark")
    : theme;
  document.documentElement.dataset.theme = resolved;
  editor.setAttribute("theme", resolved);
}

function updateDocumentName() {
  documentName.textContent = currentAssociation?.name ?? "Host-managed note";
  document.title = `${documentName.textContent} — Continuity Web`;
}

function setStatus(message) {
  status.textContent = message;
}

function showBanner(message, actions = []) {
  bannerText.textContent = message;
  bannerActions.replaceChildren(...actions);
  banner.hidden = false;
}

function hideBanner() {
  banner.hidden = true;
  bannerActions.replaceChildren();
}

function action(label, listener) {
  const button = document.createElement("button");
  button.type = "button";
  button.textContent = label;
  button.addEventListener("click", () => void Promise.resolve(listener()).catch(showHostError));
  return button;
}

function showHostError(error) {
  showBanner(String(error?.message ?? error));
}

async function runSmoke(run) {
  if (run === 2 && !initial.recovery.recovered) {
    throw new Error("interrupted-write recovery was not reported on the second launch");
  }
  if (run === 2 && !editor.snapshot().text.includes("desktop-smoke-one")) {
    throw new Error("the second launch did not restore the first launch edit");
  }
  editor.focus();
  const input = editor.shadowRoot.querySelector("textarea");
  if (input.getAttribute("aria-label") !== "Continuity markdown document"
      || input.getAttribute("aria-multiline") !== "true") {
    throw new Error("the packaged editor lost its semantic textbox contract");
  }
  input.setSelectionRange(input.value.length, input.value.length);
  input.dispatchEvent(new InputEvent("beforeinput", {
    bubbles: true,
    cancelable: true,
    data: `\ndesktop-smoke-${run === 1 ? "one" : "two"}`,
    inputType: "insertText",
  }));
  await latestPersistence;
  await exportDocument(true);
  if (run === 1) {
    await host.smokeInterrupt();
  }
  host.smokeComplete(editor.snapshot(), { recovered: initial.recovery.recovered });
}

function runCloseProbe() {
  editor.focus();
  const input = editor.shadowRoot.querySelector("textarea");
  input.setSelectionRange(input.value.length, input.value.length);
  input.dispatchEvent(new InputEvent("beforeinput", {
    bubbles: true,
    cancelable: true,
    data: "\ndesktop-close-probe",
    inputType: "insertText",
  }));
  host.smokeRequestQuit();
}
