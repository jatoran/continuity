const { contextBridge, ipcRenderer } = require("electron");

function subscribe(channel, listener) {
  const wrapped = (_event, detail) => listener(detail);
  ipcRenderer.on(channel, wrapped);
  return () => ipcRenderer.removeListener(channel, wrapped);
}

contextBridge.exposeInMainWorld("continuityDesktop", Object.freeze({
  load: () => ipcRenderer.invoke("continuity:load"),
  wasm: () => ipcRenderer.invoke("continuity:wasm"),
  persist: (detail) => ipcRenderer.invoke("continuity:persist", detail),
  persistMetadata: (snapshot) => ipcRenderer.invoke("continuity:persist-metadata", snapshot),
  newDocument: (snapshot) => ipcRenderer.invoke("continuity:new-document", snapshot),
  openDocument: () => ipcRenderer.invoke("continuity:open-document"),
  exportDocument: (snapshot, saveAs) => ipcRenderer.invoke("continuity:export-document", snapshot, Boolean(saveAs)),
  saveSettings: (settings) => ipcRenderer.invoke("continuity:save-settings", settings),
  acceptExternal: (mode, snapshot) => ipcRenderer.invoke("continuity:accept-external", mode, snapshot),
  openLink: (href) => ipcRenderer.invoke("continuity:open-link", href),
  copyText: (text) => ipcRenderer.invoke("continuity:copy-text", text),
  checkForUpdates: () => ipcRenderer.invoke("continuity:check-updates"),
  installUpdate: () => ipcRenderer.send("continuity:install-update"),
  setEditorFocused: (focused) => ipcRenderer.send("continuity:editor-focus", Boolean(focused)),
  rendererReady: () => ipcRenderer.send("continuity:renderer-ready"),
  readyToClose: () => ipcRenderer.send("continuity:ready-to-close"),
  onMenuCommand: (listener) => subscribe("continuity:menu-command", listener),
  onEditorCommand: (listener) => subscribe("continuity:editor-command", listener),
  onExternalChange: (listener) => subscribe("continuity:external-change", listener),
  onUpdateStatus: (listener) => subscribe("continuity:update-status", listener),
  onBeforeClose: (listener) => subscribe("continuity:before-close", listener),
  smokeRun: Number(process.env.CONTINUITY_DESKTOP_SMOKE_RUN ?? 0),
  closeProbe: process.env.CONTINUITY_DESKTOP_CLOSE_PROBE === "1",
  smokeInterrupt: () => ipcRenderer.invoke("continuity:smoke-interrupt"),
  smokeRequestQuit: () => ipcRenderer.send("continuity:smoke-request-quit"),
  smokeComplete: (snapshot, detail) => ipcRenderer.send("continuity:smoke-complete", snapshot, detail),
}));
