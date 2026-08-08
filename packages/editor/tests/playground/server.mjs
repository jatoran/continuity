import { createServer } from "node:http";
import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { extname, join, normalize, resolve } from "node:path";
import { networkInterfaces } from "node:os";

const root = resolve(process.argv[2] ?? ".");
const port = Number(process.argv[3] ?? 8787);

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".json": "application/json",
  ".wasm": "application/wasm",
  ".css": "text/css; charset=utf-8",
  ".map": "application/json",
};

/**
 * Content hash of everything that can change between deploys. Phone browsers
 * hold ES modules firmly enough that `Cache-Control: no-store` alone does not
 * guarantee a reload picks up new code, so the build id goes into the module
 * URLs themselves — a stale load becomes impossible rather than discouraged.
 * It is also rendered on the page, so "did it actually update?" is answerable
 * by looking instead of by trusting.
 */
function computeBuildId() {
  const hash = createHash("sha1");
  const walk = (directory) => {
    const entries = readdirSync(directory, { withFileTypes: true })
      .sort((left, right) => left.name.localeCompare(right.name));
    for (const entry of entries) {
      if (entry.name.startsWith(".") || entry.name === "internal") continue;
      const path = join(directory, entry.name);
      if (entry.isDirectory()) continue;
      if (![".mjs", ".js", ".html"].includes(extname(entry.name))) continue;
      hash.update(entry.name);
      hash.update(readFileSync(path));
    }
  };
  for (const directory of ["", "fixed/src", "baseline/src"]) {
    const path = join(root, directory);
    if (existsSync(path)) walk(path);
  }
  return hash.digest("hex").slice(0, 10);
}

// Hashing the tree on every request would re-read it per module; recompute at
// most a few times a second so a redeploy is still picked up promptly.
let cachedBuildId = "";
let cachedAt = 0;
function currentBuildId() {
  const now = Date.now();
  if (now - cachedAt > 500) {
    cachedBuildId = computeBuildId();
    cachedAt = now;
  }
  return cachedBuildId;
}

/** Append the build id to every relative module specifier in a source file. */
function stampModuleSpecifiers(source, buildId) {
  return source.replace(
    /(\bfrom\s*|\bimport\s*\(\s*)(["'])(\.{1,2}\/[^"'?]+?\.js)(["'])/gu,
    (_match, lead, openQuote, specifier, closeQuote) => (
      `${lead}${openQuote}${specifier}?v=${buildId}${closeQuote}`
    ),
  );
}

const server = createServer((request, response) => {
  const pathname = decodeURIComponent(new URL(request.url, "http://localhost").pathname);
  const relative = pathname === "/" ? "index.html" : pathname.slice(1);
  const path = normalize(join(root, relative));
  if (!path.startsWith(root) || !existsSync(path) || statSync(path).isDirectory()) {
    response.writeHead(404).end("not found");
    return;
  }
  response.setHeader("Content-Type", TYPES[extname(path)] ?? "application/octet-stream");
  response.setHeader("Cache-Control", "no-store, no-cache, must-revalidate, max-age=0");
  // Do not let this document be the thing that refuses the clipboard. A page can
  // only restrict itself, never grant what an embedding frame withheld, so this
  // removes one variable when diagnosing a blocked paste.
  response.setHeader("Permissions-Policy", "clipboard-read=*, clipboard-write=*");
  response.setHeader("Pragma", "no-cache");
  response.setHeader("Expires", "0");
  if (extname(path) === ".html") {
    // Stamp the entry document so every module URL beneath it changes with content.
    response.end(readFileSync(path, "utf8").replaceAll("__BUILD__", currentBuildId()));
    return;
  }
  if ([".js", ".mjs"].includes(extname(path))) {
    // A stamped entry module is not enough on its own: `index.js` re-exports
    // `./src/component.js` with a bare relative specifier, and those transitive
    // URLs would still resolve to whatever the phone already cached. Stamp every
    // relative specifier on the way out so the whole graph turns over together.
    response.end(stampModuleSpecifiers(readFileSync(path, "utf8"), currentBuildId()));
    return;
  }
  response.end(readFileSync(path));
});

server.listen(port, "0.0.0.0", () => {
  const addresses = Object.values(networkInterfaces())
    .flat()
    .filter((entry) => entry && entry.family === "IPv4" && !entry.internal)
    .map((entry) => entry.address);
  process.stdout.write(`Continuity mobile playground on port ${port} (build ${computeBuildId()})\n`);
  for (const address of addresses) {
    process.stdout.write(`  http://${address}:${port}/\n`);
  }
  process.stdout.write("  ?build=baseline to load the unfixed build\n");
});
