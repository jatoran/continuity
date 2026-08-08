import { createServer } from "node:http";
import { existsSync, readFileSync } from "node:fs";
import { extname, join, normalize, resolve } from "node:path";

const root = resolve(process.argv[2] ?? ".");
const server = createServer((request, response) => {
  const pathname = decodeURIComponent(new URL(request.url, "http://localhost").pathname);
  const relative = pathname === "/" ? "manual.html" : pathname.slice(1);
  const path = normalize(join(root, relative));
  if (!path.startsWith(root) || !existsSync(path)) {
    response.writeHead(404).end("not found");
    return;
  }
  response.setHeader("Content-Type", ({
    ".html": "text/html; charset=utf-8",
    ".js": "text/javascript; charset=utf-8",
    ".mjs": "text/javascript; charset=utf-8",
    ".wasm": "application/wasm",
    ".css": "text/css; charset=utf-8",
  })[extname(path)] ?? "application/octet-stream");
  response.end(readFileSync(path));
});

server.listen(0, "127.0.0.1", () => {
  const address = server.address();
  process.stdout.write(`Continuity manual browser check: http://127.0.0.1:${address.port}/manual.html\n`);
  process.stdout.write("Press Ctrl+C when finished.\n");
});
