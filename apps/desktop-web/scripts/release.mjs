import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { validateReleaseEnvironment } from "../src/release_signing.mjs";

const action = process.argv[2];
if (!new Set(["make", "publish"]).has(action)) {
  throw new Error("usage: node scripts/release.mjs <make|publish>");
}
validateReleaseEnvironment(process.platform, process.env);
if (action === "publish" && !process.env.GITHUB_TOKEN) {
  throw new Error("GITHUB_TOKEN is required to publish desktop artifacts");
}

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const executable = join(root, "node_modules", ".bin", process.platform === "win32"
  ? "electron-forge.cmd"
  : "electron-forge");
const result = spawnSync(executable, [action], {
  cwd: root,
  stdio: "inherit",
  env: { ...process.env, CONTINUITY_RELEASE_BUILD: "1" },
});
if (result.error) {
  throw result.error;
}
process.exit(result.status ?? 1);
