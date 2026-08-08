import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";

export function validateReleaseEnvironment(platform, environment) {
  if (platform === "win32") {
    requireValues(environment, ["WINDOWS_CERTIFICATE_FILE", "WINDOWS_CERTIFICATE_PASSWORD"]);
    if (!existsSync(environment.WINDOWS_CERTIFICATE_FILE)) {
      throw new Error("WINDOWS_CERTIFICATE_FILE does not exist");
    }
  } else if (platform === "darwin") {
    requireValues(environment, ["APPLE_ID", "APPLE_APP_SPECIFIC_PASSWORD", "APPLE_TEAM_ID"]);
  } else if (platform === "linux") {
    requireValues(environment, ["CONTINUITY_LINUX_GPG_KEY"]);
  } else {
    throw new Error(`unsupported release platform: ${platform}`);
  }
}

export function signLinuxArtifacts(makeResults, environment, platform = process.platform, runner = spawnSync) {
  if (environment.CONTINUITY_RELEASE_BUILD !== "1" || platform !== "linux") {
    return makeResults;
  }
  validateReleaseEnvironment(platform, environment);
  for (const result of makeResults) {
    const signatures = [];
    for (const artifact of result.artifacts) {
      if (artifact.endsWith(".asc")) {
        continue;
      }
      const signature = `${artifact}.asc`;
      const signed = runner("gpg", [
        "--batch",
        "--yes",
        "--armor",
        "--detach-sign",
        "--local-user",
        environment.CONTINUITY_LINUX_GPG_KEY,
        "--output",
        signature,
        artifact,
      ], { encoding: "utf8" });
      if (signed.error) {
        throw signed.error;
      }
      if (signed.status !== 0) {
        throw new Error(`Linux artifact signing failed for ${artifact}: ${signed.stderr ?? ""}`);
      }
      if (!existsSync(signature)) {
        throw new Error(`Linux artifact signature was not created: ${signature}`);
      }
      signatures.push(signature);
    }
    result.artifacts.push(...signatures);
  }
  return makeResults;
}

function requireValues(environment, names) {
  const missing = names.filter((name) => !environment[name]);
  if (missing.length > 0) {
    throw new Error(`release credentials are missing: ${missing.join(", ")}`);
  }
}
