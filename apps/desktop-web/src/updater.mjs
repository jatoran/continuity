import { app, autoUpdater } from "electron";

const UPDATE_INTERVAL_MS = 6 * 60 * 60 * 1000;
const SUPPORTED_PLATFORMS = new Set(["darwin", "win32"]);

export class ApplicationUpdater {
  #send;
  #timer;

  constructor(send) {
    this.#send = send;
  }

  start(isEnabled) {
    if (!isEnabled || !app.isPackaged || !SUPPORTED_PLATFORMS.has(process.platform)) {
      this.#emit({ state: "disabled", reason: process.platform === "linux" ? "system-package" : "development" });
      return;
    }
    const feed = `https://update.electronjs.org/continuity-editor/continuity/${process.platform}-${process.arch}/${app.getVersion()}`;
    autoUpdater.setFeedURL({ url: feed });
    autoUpdater.on("checking-for-update", () => this.#emit({ state: "checking" }));
    autoUpdater.on("update-not-available", () => this.#emit({ state: "current" }));
    autoUpdater.on("update-available", () => this.#emit({ state: "downloading" }));
    autoUpdater.on("update-downloaded", (_event, notes, name) => {
      this.#emit({ state: "ready", name, notes: typeof notes === "string" ? notes : "" });
    });
    autoUpdater.on("error", (error) => this.#emit({ state: "error", message: String(error.message ?? error) }));
    void this.check();
    this.#timer = setInterval(() => void this.check(), UPDATE_INTERVAL_MS);
    this.#timer.unref();
  }

  async check() {
    if (!app.isPackaged || !SUPPORTED_PLATFORMS.has(process.platform)) {
      this.#emit({ state: "disabled", reason: process.platform === "linux" ? "system-package" : "development" });
      return;
    }
    await autoUpdater.checkForUpdates();
  }

  install() {
    autoUpdater.quitAndInstall();
  }

  stop() {
    if (this.#timer) {
      clearInterval(this.#timer);
      this.#timer = undefined;
    }
  }

  #emit(detail) {
    this.#send("continuity:update-status", detail);
  }
}
