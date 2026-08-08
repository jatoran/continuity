import { createHash } from "node:crypto";
import { watch } from "node:fs";
import { open, readFile, rename, rm, stat } from "node:fs/promises";
import { basename } from "node:path";

const MAX_IMPORT_BYTES = 50 * 1024 * 1024;

export async function loadTextFile(path) {
  const metadata = await stat(path);
  if (metadata.size > MAX_IMPORT_BYTES) {
    throw new Error("files larger than 50 MiB are not accepted by the preview shell");
  }
  const bytes = await readFile(path);
  const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  return {
    text,
    association: {
      path,
      name: basename(path),
      modifiedAt: metadata.mtimeMs,
      contentHash: hashBytes(bytes),
    },
  };
}

export async function exportTextFile(path, text) {
  const temporary = `${path}.continuity-next`;
  const previous = `${path}.continuity-previous`;
  const handle = await open(temporary, "w");
  try {
    await handle.writeFile(text, "utf8");
    await handle.sync();
  } finally {
    await handle.close();
  }
  await rm(previous, { force: true });
  let hadPrevious = false;
  try {
    await rename(path, previous);
    hadPrevious = true;
  } catch (error) {
    if (error?.code !== "ENOENT") {
      throw error;
    }
  }
  try {
    await rename(temporary, path);
  } catch (error) {
    if (hadPrevious) {
      await rename(previous, path);
    }
    throw error;
  }
  await rm(previous, { force: true });
  return loadTextFile(path);
}

export function hashText(text) {
  return hashBytes(Buffer.from(text, "utf8"));
}

export function watchTextFile(path, listener, debounceMs = 120) {
  let timer;
  const watcher = watch(path, { persistent: false }, () => {
    clearTimeout(timer);
    timer = setTimeout(async () => {
      try {
        listener({ opened: await loadTextFile(path) });
      } catch (error) {
        listener({ error });
      }
    }, debounceMs);
    timer.unref?.();
  });
  return {
    close() {
      clearTimeout(timer);
      watcher.close();
    },
  };
}

function hashBytes(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}
