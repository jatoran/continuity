import { spawnSync } from "node:child_process";

export const WINDOWS_DOCUMENT_PROG_ID = "ContinuityWeb.Markdown";
export const WINDOWS_REGISTERED_APPLICATION = "Continuity Web";

const CLASSES = "HKCU\\Software\\Classes";
const APPLICATION = "HKCU\\Software\\Continuity Web";
const REGISTERED_APPLICATIONS = "HKCU\\Software\\RegisteredApplications";

export function windowsRegistrationCommands(executable, mode) {
  if (mode === "install") {
    const quotedCommand = `\"${executable}\" \"%1\"`;
    return [
      add(`${CLASSES}\\${WINDOWS_DOCUMENT_PROG_ID}`, undefined, "Continuity Web Markdown document"),
      add(`${CLASSES}\\${WINDOWS_DOCUMENT_PROG_ID}\\DefaultIcon`, undefined, `\"${executable}\",0`),
      add(`${CLASSES}\\${WINDOWS_DOCUMENT_PROG_ID}\\shell\\open\\command`, undefined, quotedCommand),
      add(`${CLASSES}\\.md\\OpenWithProgids`, WINDOWS_DOCUMENT_PROG_ID, ""),
      add(`${CLASSES}\\.markdown\\OpenWithProgids`, WINDOWS_DOCUMENT_PROG_ID, ""),
      add(`${APPLICATION}\\Capabilities`, "ApplicationName", WINDOWS_REGISTERED_APPLICATION),
      add(`${APPLICATION}\\Capabilities`, "ApplicationDescription", "Continuity Markdown editor"),
      add(`${APPLICATION}\\Capabilities\\FileAssociations`, ".md", WINDOWS_DOCUMENT_PROG_ID),
      add(`${APPLICATION}\\Capabilities\\FileAssociations`, ".markdown", WINDOWS_DOCUMENT_PROG_ID),
      add(REGISTERED_APPLICATIONS, WINDOWS_REGISTERED_APPLICATION, "Software\\Continuity Web\\Capabilities"),
    ];
  }
  if (mode === "uninstall") {
    return [
      removeValue(`${CLASSES}\\.md\\OpenWithProgids`, WINDOWS_DOCUMENT_PROG_ID),
      removeValue(`${CLASSES}\\.markdown\\OpenWithProgids`, WINDOWS_DOCUMENT_PROG_ID),
      removeValue(REGISTERED_APPLICATIONS, WINDOWS_REGISTERED_APPLICATION),
      removeKey(APPLICATION),
      removeKey(`${CLASSES}\\${WINDOWS_DOCUMENT_PROG_ID}`),
    ];
  }
  throw new Error(`unsupported Windows registration mode: ${mode}`);
}

export function applyWindowsRegistration(executable, mode, runner = spawnSync) {
  for (const command of windowsRegistrationCommands(executable, mode)) {
    const result = runner("reg.exe", command.args, {
      windowsHide: true,
      encoding: "utf8",
    });
    if (result.error) {
      throw result.error;
    }
    if (result.status !== 0 && !command.mayBeMissing) {
      throw new Error(`Windows registration failed: reg.exe ${command.args.join(" ")}\n${result.stderr ?? ""}`);
    }
  }
}

function add(key, valueName, data) {
  const value = valueName ? ["/v", valueName] : ["/ve"];
  return { args: ["add", key, ...value, "/t", "REG_SZ", "/d", data, "/f"], mayBeMissing: false };
}

function removeValue(key, valueName) {
  return { args: ["delete", key, "/v", valueName, "/f"], mayBeMissing: true };
}

function removeKey(key) {
  return { args: ["delete", key, "/f"], mayBeMissing: true };
}
