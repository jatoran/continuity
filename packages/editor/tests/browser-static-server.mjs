import { createServer } from "node:http";
import { existsSync, readFileSync } from "node:fs";
import { extname, join, normalize } from "node:path";

/** Serve one generated packed-browser consumer with strict local MIME types. */
export function createStaticServer(directory) {
  return createServer((request, response) => {
    const pathname = decodeURIComponent(new URL(request.url, "http://localhost").pathname);
    const relative = pathname === "/" ? "browser.html" : pathname.slice(1);
    const path = normalize(join(directory, relative));
    if (!path.startsWith(directory) || !existsSync(path)) {
      response.writeHead(404).end("not found");
      return;
    }
    response.setHeader("Content-Type", contentType(path));
    response.end(readFileSync(path));
  });
}

function contentType(path) {
  return ({
    ".html": "text/html; charset=utf-8",
    ".js": "text/javascript; charset=utf-8",
    ".mjs": "text/javascript; charset=utf-8",
    ".json": "application/json",
    ".wasm": "application/wasm",
    ".css": "text/css; charset=utf-8",
  })[extname(path)] ?? "application/octet-stream";
}
