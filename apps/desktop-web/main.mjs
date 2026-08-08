import { existsSync } from "node:fs";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  app,
  BrowserWindow,
  clipboard,
  dialog,
  ipcMain,
  shell,
} from "electron";
import squirrelStartup from "electron-squirrel-startup";

import { exportTextFile, loadTextFile, watchTextFile } from "./src/files.mjs";
import { installApplicationMenu } from "./src/menu.mjs";
import { installApplicationProtocol } from "./src/protocol.mjs";
import { loadSettings, saveSettings } from "./src/settings.mjs";
import { DurableDocumentStore } from "./src/storage.mjs";
import { ApplicationUpdater } from "./src/updater.mjs";
import { applyWindowsRegistration } from "./src/windows_registration.mjs";

const processStartedAt = process.hrtime.bigint();
const root = dirname(fileURLToPath(import.meta.url));
const instanceProbeDirectory = process.env.CONTINUITY_DESKTOP_INSTANCE_PROBE;
const isCloseProbe = process.env.CONTINUITY_DESKTOP_CLOSE_PROBE === "1";
const isSmoke = process.argv.includes("--smoke")
  || process.env.CONTINUITY_DESKTOP_SMOKE_RUN !== undefined
  || instanceProbeDirectory !== undefined
  || isCloseProbe;
const smokeRun = Number(process.env.CONTINUITY_DESKTOP_SMOKE_RUN ?? 0);
const disposableUserData = process.env.CONTINUITY_DESKTOP_USER_DATA;

if (disposableUserData) {
  app.setPath("userData", resolve(disposableUserData));
}
if (isSmoke) {
  app.disableHardwareAcceleration();
  app.commandLine.appendSwitch("headless");
  app.commandLine.appendSwitch("disable-gpu");
  app.commandLine.appendSwitch("no-sandbox");
}

let window;
let store;
let settings;
let currentAssociation;
let pendingExternal;
let associationWatcher;
let isEditorFocused = false;
let isRendererReady = false;
let canClose = false;
let isCloseRequested = false;
let isQuitRequested = false;
let isUpdateInstallRequested = false;
let updater;
let startupDocumentPath = findStartupDocument(process.argv.slice(1));

applySquirrelRegistration();

if (squirrelStartup) {
  app.quit();
} else if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  installIpcHandlers();
  installApplicationEvents();
  void start().catch((error) => {
    process.stderr.write(`continuity desktop startup failed: ${error.stack ?? error}\n`);
    app.exit(1);
  });
}

function installApplicationEvents() {
  app.on("open-file", (event, path) => {
    event.preventDefault();
    receiveDocumentPath(path);
  });
  app.on("second-instance", (_event, commandLine) => {
    const path = findStartupDocument(commandLine);
    if (path) {
      receiveDocumentPath(path);
    }
    if (window) {
      if (window.isMinimized()) {
        window.restore();
      }
      window.show();
      window.focus();
    }
    if (instanceProbeDirectory) {
      void writeFile(join(instanceProbeDirectory, "second-received.json"), JSON.stringify(commandLine))
        .then(() => app.exit(0));
    }
  });
  app.on("window-all-closed", () => app.quit());
  app.on("before-quit", (event) => {
    if (!canClose && window && !window.isDestroyed()) {
      event.preventDefault();
      isQuitRequested = true;
      requestRendererClose();
      return;
    }
    closeHostServices();
  });
}

function receiveDocumentPath(path) {
  startupDocumentPath = path;
  if (window && isRendererReady) {
    startupDocumentPath = undefined;
    void openPathInRenderer(path);
  }
}

function requestRendererClose() {
  if (!isCloseRequested) {
    isCloseRequested = true;
    sendToRenderer("continuity:before-close", undefined);
  }
}

function closeHostServices() {
  associationWatcher?.close();
  updater?.stop();
}

function applySquirrelRegistration() {
  if (process.platform !== "win32") {
    return;
  }
  const event = process.argv[1];
  const mode = ["--squirrel-install", "--squirrel-updated"].includes(event)
    ? "install"
    : event === "--squirrel-uninstall" ? "uninstall" : undefined;
  if (mode) {
    applyWindowsRegistration(process.execPath, mode);
  }
}

async function start() {
  await app.whenReady();
  await mkdir(join(app.getPath("userData"), "documents"), { recursive: true });
  store = new DurableDocumentStore(join(app.getPath("userData"), "documents"));
  const initialDocument = await store.load();
  currentAssociation = initialDocument.associatedFile;
  settings = await loadSettings(join(app.getPath("userData"), "settings.json"));
  installApplicationProtocol(root);
  window = createWindow();
  updater = new ApplicationUpdater(sendToRenderer);
  installApplicationMenu(sendToRenderer, settings.theme);
  watchAssociation();
  await window.loadURL("continuity://app/index.html");
  if (instanceProbeDirectory) {
    await mkdir(instanceProbeDirectory, { recursive: true });
    await writeFile(join(instanceProbeDirectory, "primary-ready"), "ready\n", "utf8");
  }
}

function createWindow() {
  const nextWindow = new BrowserWindow({
    width: 1060,
    height: 760,
    minWidth: 560,
    minHeight: 420,
    show: !isSmoke,
    backgroundColor: "#111318",
    webPreferences: {
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      preload: join(root, "preload.cjs"),
    },
  });
  nextWindow.webContents.setWindowOpenHandler(() => ({ action: "deny" }));
  nextWindow.webContents.on("will-navigate", (event, url) => {
    if (!url.startsWith("continuity://app/")) {
      event.preventDefault();
    }
  });
  nextWindow.webContents.session.setPermissionRequestHandler((_contents, _permission, callback) => callback(false));
  nextWindow.webContents.on("before-input-event", (event, input) => {
    const command = isEditorFocused ? editorCommandForInput(input) : undefined;
    if (command) {
      event.preventDefault();
      sendToRenderer("continuity:editor-command", command);
    }
  });
  nextWindow.on("close", (event) => {
    if (!canClose) {
      event.preventDefault();
      requestRendererClose();
    }
  });
  nextWindow.webContents.on("render-process-gone", (_event, detail) => {
    isRendererReady = false;
    process.stderr.write(`continuity desktop renderer exited: ${JSON.stringify(detail)}\n`);
  });
  nextWindow.webContents.on("console-message", (detail) => {
    if (isSmoke) {
      process.stderr.write(`continuity desktop renderer ${detail.level}: ${detail.message}\n`);
    }
  });
  nextWindow.webContents.on("did-fail-load", (_event, code, description, url) => {
    process.stderr.write(`continuity desktop load failed ${code} at ${url}: ${description}\n`);
  });
  nextWindow.webContents.on("preload-error", (_event, path, error) => {
    process.stderr.write(`continuity desktop preload failed at ${path}: ${error.stack ?? error}\n`);
  });
  if (isSmoke) {
    const timeout = setTimeout(async () => {
      const diagnostics = await nextWindow.webContents.executeJavaScript(`(async () => ({
        title: document.title,
        body: document.body?.innerText,
        bridge: typeof window.continuityDesktop,
        editor: document.querySelector('continuity-editor')?.snapshot?.(),
        resources: performance.getEntriesByType('resource').map(({ name }) => name),
        importError: await import('./renderer.mjs').then(() => null).catch((error) => String(error.stack ?? error)),
      }))()`).catch((error) => ({ error: String(error) }));
      process.stderr.write(`continuity desktop smoke timed out: ${JSON.stringify(diagnostics)}\n`);
      app.exit(1);
    }, 15_000);
    timeout.unref();
  }
  return nextWindow;
}

function installIpcHandlers() {
  ipcMain.handle("continuity:load", async (event) => {
    assertTrusted(event);
    const document = await store.load();
    currentAssociation = document.associatedFile;
    watchAssociation();
    return { ...document, settings, platform: process.platform, appVersion: app.getVersion() };
  });
  ipcMain.handle("continuity:wasm", async (event) => {
    assertTrusted(event);
    return readFile(fileURLToPath(import.meta.resolve("@continuity-editor/editor/wasm")));
  });
  ipcMain.handle("continuity:persist", async (event, detail) => {
    assertTrusted(event);
    return store.persist(detail, currentAssociation);
  });
  ipcMain.handle("continuity:persist-metadata", async (event, snapshot) => {
    assertTrusted(event);
    validateSnapshot(snapshot);
    return store.persistMetadata(snapshot, currentAssociation);
  });
  ipcMain.handle("continuity:new-document", async (event, snapshot) => {
    assertTrusted(event);
    validateSnapshot(snapshot);
    currentAssociation = null;
    pendingExternal = undefined;
    watchAssociation();
    return store.persistMetadata(snapshot, null);
  });
  ipcMain.handle("continuity:open-document", async (event) => {
    assertTrusted(event);
    const result = await dialog.showOpenDialog(window, {
      properties: ["openFile"],
      filters: [
        { name: "Markdown and text", extensions: ["md", "markdown", "txt"] },
        { name: "All files", extensions: ["*"] },
      ],
    });
    return result.canceled ? null : adoptAssociation(await loadTextFile(result.filePaths[0]));
  });
  ipcMain.handle("continuity:export-document", async (event, snapshot, saveAs) => {
    assertTrusted(event);
    validateSnapshot(snapshot);
    let path = !saveAs ? currentAssociation?.path : undefined;
    if (!path && isSmoke) {
      path = join(app.getPath("userData"), "smoke-export.md");
    }
    if (!path) {
      const result = await dialog.showSaveDialog(window, {
        defaultPath: currentAssociation?.path ?? "continuity.md",
        filters: [{ name: "Markdown", extensions: ["md"] }],
      });
      if (result.canceled || !result.filePath) {
        return null;
      }
      path = result.filePath;
    }
    const exported = await exportTextFile(path, snapshot.text);
    currentAssociation = exported.association;
    pendingExternal = undefined;
    watchAssociation();
    const durable = await store.persistMetadata(snapshot, currentAssociation);
    return { ...durable, association: currentAssociation };
  });
  ipcMain.handle("continuity:save-settings", async (event, nextSettings) => {
    assertTrusted(event);
    settings = await saveSettings(join(app.getPath("userData"), "settings.json"), nextSettings);
    return settings;
  });
  ipcMain.handle("continuity:accept-external", async (event, mode, snapshot) => {
    assertTrusted(event);
    validateSnapshot(snapshot);
    if (!pendingExternal || !["keep", "reload"].includes(mode)) {
      throw new Error("no matching external change is pending");
    }
    currentAssociation = pendingExternal.association;
    const accepted = pendingExternal;
    pendingExternal = undefined;
    watchAssociation();
    if (mode === "keep") {
      await store.persistMetadata(snapshot, currentAssociation);
    }
    return accepted;
  });
  ipcMain.handle("continuity:open-link", async (event, href) => {
    assertTrusted(event);
    const url = new URL(href);
    if (!new Set(["http:", "https:", "mailto:"]).has(url.protocol)) {
      throw new Error("unsupported external link protocol");
    }
    await shell.openExternal(url.href);
  });
  ipcMain.handle("continuity:copy-text", (event, text) => {
    assertTrusted(event);
    if (typeof text !== "string") {
      throw new Error("clipboard text must be a string");
    }
    clipboard.writeText(text);
  });
  ipcMain.handle("continuity:check-updates", async (event) => {
    assertTrusted(event);
    await updater.check();
  });
  ipcMain.handle("continuity:smoke-interrupt", async (event) => {
    assertTrusted(event);
    if (!isSmoke) {
      throw new Error("smoke interruption is unavailable outside the test host");
    }
    await store.simulateInterruptedWrite();
  });
  ipcMain.on("continuity:smoke-request-quit", (event) => {
    assertTrusted(event);
    if (!isCloseProbe) {
      throw new Error("close probe is unavailable outside its test host");
    }
    app.quit();
  });
  ipcMain.on("continuity:install-update", (event) => {
    assertTrusted(event);
    isUpdateInstallRequested = true;
    requestRendererClose();
  });
  ipcMain.on("continuity:editor-focus", (event, focused) => {
    assertTrusted(event);
    isEditorFocused = Boolean(focused);
  });
  ipcMain.on("continuity:renderer-ready", (event) => {
    assertTrusted(event);
    isRendererReady = true;
    updater.start(settings.checkForUpdates && !isSmoke);
    if (startupDocumentPath) {
      const path = startupDocumentPath;
      startupDocumentPath = undefined;
      void openPathInRenderer(path);
    }
  });
  ipcMain.on("continuity:ready-to-close", (event) => {
    assertTrusted(event);
    if (isCloseProbe) {
      void completeCloseProbe();
      return;
    }
    canClose = true;
    if (isUpdateInstallRequested) {
      updater.install();
    } else if (isQuitRequested) {
      app.quit();
    } else {
      window.close();
    }
  });
  ipcMain.on("continuity:smoke-complete", (event, snapshot, detail) => {
    assertTrusted(event);
    void completeSmoke(snapshot, detail);
  });
}

async function completeCloseProbe() {
  try {
    const restored = await new DurableDocumentStore(join(app.getPath("userData"), "documents")).load();
    if (!restored.text.includes("desktop-close-probe")) {
      throw new Error("quit handshake completed before the final edit became durable");
    }
    process.stdout.write("CONTINUITY_DESKTOP_CLOSE PASS\n");
    canClose = true;
    app.quit();
  } catch (error) {
    process.stderr.write(`continuity desktop close probe failed: ${error.stack ?? error}\n`);
    app.exit(1);
  }
}

async function openPathInRenderer(path) {
  try {
    const opened = adoptAssociation(await loadTextFile(path));
    sendToRenderer("continuity:menu-command", { command: "open-path", opened });
  } catch (error) {
    sendToRenderer("continuity:menu-command", { command: "host-error", message: String(error.message ?? error) });
  }
}

function adoptAssociation(opened) {
  currentAssociation = opened.association;
  pendingExternal = undefined;
  watchAssociation();
  return opened;
}

function watchAssociation() {
  associationWatcher?.close();
  associationWatcher = undefined;
  if (!currentAssociation?.path || !existsSync(currentAssociation.path)) {
    return;
  }
  const watchedPath = currentAssociation.path;
  associationWatcher = watchTextFile(watchedPath, ({ opened, error }) => {
    if (error) {
      sendToRenderer("continuity:external-change", {
        error: String(error.message ?? error),
        association: currentAssociation,
      });
    } else if (currentAssociation?.path === watchedPath
        && opened.association.contentHash !== currentAssociation.contentHash) {
      pendingExternal = opened;
      sendToRenderer("continuity:external-change", opened);
    }
  });
}

async function completeSmoke(snapshot, detail) {
  try {
    validateSnapshot(snapshot);
    const restored = await new DurableDocumentStore(join(app.getPath("userData"), "documents")).load();
    const exported = await readFile(join(app.getPath("userData"), "smoke-export.md"), "utf8");
    if (restored.text !== snapshot.text || restored.revision !== snapshot.revision
        || exported !== snapshot.text) {
      throw new Error("packaged smoke persistence/export does not match the renderer snapshot");
    }
    if (smokeRun === 2 && !detail?.recovered) {
      throw new Error("second smoke launch did not report interrupted-write recovery");
    }
    const workingSetBytes = app.getAppMetrics()
      .reduce((sum, metric) => sum + metric.memory.workingSetSize * 1024, 0);
    const metrics = {
      platform: process.platform,
      arch: process.arch,
      electron: process.versions.electron,
      run: smokeRun,
      startupMs: Number(process.hrtime.bigint() - processStartedAt) / 1_000_000,
      workingSetBytes,
      revision: restored.revision,
      durableSequence: restored.durableSequence,
      recovered: restored.recovery.recovered,
    };
    process.stdout.write(`CONTINUITY_DESKTOP_METRICS ${JSON.stringify(metrics)}\n`);
    setTimeout(() => app.exit(0), 25);
  } catch (error) {
    process.stderr.write(`continuity desktop smoke failed: ${error.stack ?? error}\n`);
    setTimeout(() => app.exit(1), 25);
  }
}

function sendToRenderer(channel, detail) {
  if (window && !window.isDestroyed()) {
    window.webContents.send(channel, detail);
  }
}

function assertTrusted(event) {
  if (!event.senderFrame?.url.startsWith("continuity://app/")) {
    throw new Error("rejected IPC from an untrusted renderer");
  }
}

function validateSnapshot(snapshot) {
  if (typeof snapshot?.text !== "string" || !Number.isSafeInteger(snapshot.revision)
      || snapshot.revision < 0 || !Array.isArray(snapshot.selections)) {
    throw new Error("invalid editor snapshot");
  }
}

function findStartupDocument(args) {
  return args.find((argument) => !argument.startsWith("-")
    && [".md", ".markdown", ".txt"].includes(extname(argument).toLowerCase())
    && existsSync(argument));
}

function editorCommandForInput(input) {
  if (input.type !== "keyDown" || input.alt || input.control === input.meta) {
    return undefined;
  }
  const key = input.key.toLowerCase();
  return ({
    e: "markdown.toggle_task",
    j: "editor.join_lines",
    k: "markdown.insert_link",
    r: "editor.toggle_bullet_at_line_start",
    "shift+c": "markdown.insert_code_fence",
    "shift+j": "editor.join_selected_lines",
    "shift+r": "editor.toggle_bullet_indent_continuation",
    u: "editor.change_case_upper",
  })[`${input.shift ? "shift+" : ""}${key}`];
}
