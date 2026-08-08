import { resolve, sep } from "node:path";
import { pathToFileURL } from "node:url";

import { net, protocol } from "electron";

protocol.registerSchemesAsPrivileged([{
  scheme: "continuity",
  privileges: {
    standard: true,
    secure: true,
    supportFetchAPI: true,
    corsEnabled: false,
  },
}]);

export function installApplicationProtocol(root) {
  const applicationRoot = resolve(root);
  protocol.handle("continuity", async (request) => {
    const url = new URL(request.url);
    if (url.hostname !== "app") {
      return new Response("Not found", { status: 404 });
    }
    const relative = decodeURIComponent(url.pathname).replace(/^\/+/, "") || "index.html";
    const path = resolve(applicationRoot, relative);
    if (path !== applicationRoot && !path.startsWith(`${applicationRoot}${sep}`)) {
      return new Response("Forbidden", { status: 403 });
    }
    try {
      return await net.fetch(pathToFileURL(path).href);
    } catch (error) {
      process.stderr.write(`continuity protocol failed ${request.url} -> ${path}: ${error}\n`);
      if (error?.code === "ENOENT") {
        return new Response("Not found", { status: 404 });
      }
      throw error;
    }
  });
}
