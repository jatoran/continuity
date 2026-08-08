import { createHash } from "node:crypto";
import { readdir, readFile, writeFile } from "node:fs/promises";
import { join, relative, resolve } from "node:path";

const root = resolve(process.argv[2]);
const files = await collect(root);
const integrity = {};
for (const path of files.sort()) {
  const name = relative(root, path).replaceAll("\\", "/");
  if (name === "integrity.json") {
    continue;
  }
  const digest = createHash("sha384").update(await readFile(path)).digest("base64");
  integrity[name] = `sha384-${digest}`;
}
await writeFile(
  join(root, "integrity.json"),
  `${JSON.stringify({ algorithm: "sha384", files: integrity }, null, 2)}\n`,
  "utf8",
);

async function collect(directory) {
  const paths = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      paths.push(...await collect(path));
    } else if (entry.isFile()) {
      paths.push(path);
    }
  }
  return paths;
}
