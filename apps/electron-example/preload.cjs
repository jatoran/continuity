const { contextBridge, ipcRenderer } = require("electron");

contextBridge.exposeInMainWorld("continuityHost", Object.freeze({
  load: () => ipcRenderer.invoke("continuity:load"),
  persist: (detail) => ipcRenderer.invoke("continuity:persist", detail),
  wasm: () => ipcRenderer.invoke("continuity:wasm"),
  setEditorFocused: (focused) => ipcRenderer.send("continuity:editor-focus", focused),
  onEditorCommand: (listener) => ipcRenderer.on("continuity:editor-command", (_event, command) => listener(command)),
  isSmoke: process.argv.includes("--smoke") || process.env.CONTINUITY_ELECTRON_SMOKE === "1",
  smokeComplete: (snapshot) => ipcRenderer.send("continuity:smoke-complete", snapshot),
  smokeMultiCursor: () => ipcRenderer.invoke("continuity:smoke-multicursor"),
  smokeShortcut: () => ipcRenderer.invoke("continuity:smoke-shortcut"),
  onSmokeAck: (listener) => ipcRenderer.once("continuity:smoke-ack", (_event, result) => listener(result)),
}));
