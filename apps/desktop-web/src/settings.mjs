import { mkdir, readFile, rename, rm, writeFile } from "node:fs/promises";
import { dirname } from "node:path";

export const DEFAULT_SETTINGS = Object.freeze({
  version: 1,
  theme: "system",
  checkForUpdates: true,
});

export async function loadSettings(path) {
  try {
    return normalizeSettings(JSON.parse(await readFile(path, "utf8")));
  } catch (error) {
    if (error?.code !== "ENOENT" && !(error instanceof SyntaxError)) {
      throw error;
    }
    return { ...DEFAULT_SETTINGS };
  }
}

export async function saveSettings(path, settings) {
  const normalized = normalizeSettings(settings);
  const temporary = `${path}.next`;
  await mkdir(dirname(path), { recursive: true });
  await writeFile(temporary, `${JSON.stringify(normalized, null, 2)}\n`, "utf8");
  await rm(path, { force: true });
  await rename(temporary, path);
  return normalized;
}

export function normalizeSettings(value) {
  const theme = ["system", "light", "dark"].includes(value?.theme)
    ? value.theme
    : DEFAULT_SETTINGS.theme;
  return {
    version: 1,
    theme,
    checkForUpdates: value?.checkForUpdates !== false,
  };
}
