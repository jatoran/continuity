import { app, BrowserWindow, ipcMain } from "electron";
import { appendFile, readFile, rename, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(fileURLToPath(import.meta.url));
const isSmoke = process.argv.includes("--smoke") || process.env.CONTINUITY_ELECTRON_SMOKE === "1";
const userData = process.env.CONTINUITY_ELECTRON_USER_DATA;
if (userData) {
  app.setPath("userData", userData);
}
if (isSmoke) {
  app.disableHardwareAcceleration();
  app.commandLine.appendSwitch("headless");
  app.commandLine.appendSwitch("disable-gpu");
  app.commandLine.appendSwitch("no-sandbox");
}
const documentPath = join(app.getPath("userData"), "continuity-document.json");
const smokeReportPath = join(app.getPath("userData"), "smoke-report.jsonl");
let lastRendererSequence = 0;
let lastSequence = 0;
let isEditorFocused = false;

ipcMain.handle("continuity:load", async () => {
  if (!existsSync(documentPath)) {
    return { text: "# Electron host\n\n", revision: 0, sequence: 0 };
  }
  const document = JSON.parse(await readFile(documentPath, "utf8"));
  lastSequence = document.sequence;
  return document;
});

ipcMain.handle("continuity:wasm", async () => {
  const wasmUrl = import.meta.resolve("@continuity-editor/editor/wasm");
  return readFile(fileURLToPath(wasmUrl));
});

ipcMain.on("continuity:editor-focus", (_event, focused) => {
  isEditorFocused = Boolean(focused);
});

ipcMain.handle("continuity:smoke-shortcut", async (event) => {
  if (!isSmoke) {
    throw new Error("shortcut injection is available only in smoke mode");
  }
  event.sender.sendInputEvent({
    type: "keyDown",
    keyCode: "E",
    modifiers: [process.platform === "darwin" ? "meta" : "control"],
  });
  event.sender.sendInputEvent({
    type: "keyUp",
    keyCode: "E",
    modifiers: [process.platform === "darwin" ? "meta" : "control"],
  });
});

ipcMain.handle("continuity:smoke-multicursor", async (event) => {
  if (!isSmoke) {
    throw new Error("multi-cursor injection is available only in smoke mode");
  }
  const modifiers = [process.platform === "darwin" ? "meta" : "control", "alt"];
  event.sender.sendInputEvent({ type: "keyDown", keyCode: "Down", modifiers });
  event.sender.sendInputEvent({ type: "keyUp", keyCode: "Down", modifiers });
});

ipcMain.handle("continuity:persist", async (_event, detail) => {
  if (detail.sequence <= lastRendererSequence) {
    throw new Error(`out-of-order persistence sequence ${detail.sequence}`);
  }
  const document = {
    version: detail.version,
    sequence: lastSequence + 1,
    sourceSequence: detail.sequence,
    revision: detail.snapshot.revision,
    text: detail.snapshot.text,
  };
  const temporaryPath = `${documentPath}.tmp`;
  await writeFile(temporaryPath, `${JSON.stringify(document, null, 2)}\n`, "utf8");
  await rename(temporaryPath, documentPath);
  lastRendererSequence = detail.sequence;
  lastSequence = document.sequence;
  return { sequence: lastSequence, revision: document.revision };
});

ipcMain.on("continuity:smoke-complete", async (event, expected) => {
  try {
    const persisted = JSON.parse(await readFile(documentPath, "utf8"));
    if (persisted.text !== expected.text || persisted.revision !== expected.revision) {
      throw new Error("persisted Electron document does not match renderer snapshot");
    }
    process.stdout.write(`CONTINUITY_ELECTRON_SMOKE ${JSON.stringify({
      electron: process.versions.electron,
      chrome: process.versions.chrome,
      revision: persisted.revision,
      sequence: persisted.sequence,
    })}\n`);
    await reportSmokeStage("complete", { revision: persisted.revision, sequence: persisted.sequence });
    event.sender.send("continuity:smoke-ack", { ok: true });
    setTimeout(() => app.exit(0), 25);
  } catch (error) {
    await reportSmokeStage("failed", { error: String(error.stack ?? error) });
    process.stderr.write(`continuity electron smoke failed: ${error.stack ?? error}\n`);
    event.sender.send("continuity:smoke-ack", { ok: false, error: String(error) });
    setTimeout(() => app.exit(1), 25);
  }
});

app.on("window-all-closed", () => app.quit());

async function start() {
  if (isSmoke) {
    await reportSmokeStage("main-start");
  }
  await app.whenReady();
  if (isSmoke) {
    await reportSmokeStage("app-ready");
  }
  const window = new BrowserWindow({
    width: 960,
    height: 700,
    show: !isSmoke,
    webPreferences: {
      contextIsolation: true,
      nodeIntegration: false,
      preload: join(root, "preload.cjs"),
    },
  });
  window.webContents.on("before-input-event", (event, input) => {
    const command = isEditorFocused ? editorCommandForInput(input) : undefined;
    if (command) {
      event.preventDefault();
      window.webContents.send("continuity:editor-command", command);
    }
  });
  window.webContents.on("console-message", (details) => {
    if (isSmoke) {
      process.stderr.write(`electron renderer ${details.level}: ${details.message}\n`);
      void reportSmokeStage("renderer-console", {
        level: details.level,
        message: details.message,
      });
    }
  });
  window.webContents.on("did-fail-load", (_event, code, description) => {
    process.stderr.write(`electron renderer load failed ${code}: ${description}\n`);
  });
  window.webContents.on("render-process-gone", (_event, details) => {
    process.stderr.write(`electron renderer exited: ${JSON.stringify(details)}\n`);
  });
  await window.loadFile(join(root, "index.html"));
  if (isSmoke) {
    await reportSmokeStage("page-loaded");
  }

  if (isSmoke) {
    setTimeout(async () => {
      const diagnostics = await window.webContents.executeJavaScript(`({
        body: document.body.innerText,
        host: typeof window.continuityHost,
        editor: document.querySelector('continuity-editor')?.snapshot?.(),
      })`).catch((error) => ({ diagnosticError: String(error) }));
      await reportSmokeStage("timeout", { diagnostics });
      process.stderr.write(`electron smoke timed out: ${JSON.stringify(diagnostics)}\n`);
      app.exit(1);
    }, 15_000);
  }
}

function editorCommandForInput(input) {
  if (input.type !== "keyDown" || input.alt || input.control === input.meta) {
    return undefined;
  }
  const key = input.key.toLowerCase();
  return ({
    "e": "markdown.toggle_task",
    "j": "editor.join_lines",
    "k": "markdown.insert_link",
    "r": "editor.toggle_bullet_at_line_start",
    "shift+c": "markdown.insert_code_fence",
    "shift+j": "editor.join_selected_lines",
    "shift+r": "editor.toggle_bullet_indent_continuation",
    "shift+s": "markdown.toggle_strikethrough",
    "u": "editor.change_case_upper",
  })[`${input.shift ? "shift+" : ""}${key}`];
}

void start().catch(async (error) => {
  if (isSmoke) {
    await reportSmokeStage("failed", { error: String(error.stack ?? error) });
  }
  process.stderr.write(`continuity electron startup failed: ${error.stack ?? error}\n`);
  app.exit(1);
});

async function reportSmokeStage(stage, detail = {}) {
  await appendFile(
    smokeReportPath,
    `${JSON.stringify({ stage, at: Date.now(), ...detail })}\n`,
    "utf8",
  );
}
