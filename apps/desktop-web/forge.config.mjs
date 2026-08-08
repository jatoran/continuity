import { signLinuxArtifacts, validateReleaseEnvironment } from "./src/release_signing.mjs";

const isReleaseBuild = process.env.CONTINUITY_RELEASE_BUILD === "1";
if (isReleaseBuild) {
  validateReleaseEnvironment(process.platform, process.env);
}

const macCredentials = process.env.APPLE_ID && process.env.APPLE_APP_SPECIFIC_PASSWORD
  && process.env.APPLE_TEAM_ID
  ? {
      appleId: process.env.APPLE_ID,
      appleIdPassword: process.env.APPLE_APP_SPECIFIC_PASSWORD,
      teamId: process.env.APPLE_TEAM_ID,
    }
  : undefined;

const windowsCertificate = process.env.WINDOWS_CERTIFICATE_FILE
  && process.env.WINDOWS_CERTIFICATE_PASSWORD
  ? {
      certificateFile: process.env.WINDOWS_CERTIFICATE_FILE,
      certificatePassword: process.env.WINDOWS_CERTIFICATE_PASSWORD,
    }
  : {};

export default {
  outDir: process.env.CONTINUITY_DESKTOP_OUT_DIR ?? "out",
  packagerConfig: {
    name: "Continuity Web",
    executableName: "continuity-web",
    appBundleId: "dev.continuity.editor.desktop-web",
    appCategoryType: "public.app-category.productivity",
    asar: true,
    prune: true,
    ignore: [
      /^\/out(?:[-/]|$)/,
      /^\/tests(?:\/|$)/,
      /^\/scripts(?:\/|$)/,
      /^\/package-lock\.json$/,
      /^\/README\.md$/,
    ],
    osxSign: macCredentials ? {} : undefined,
    osxNotarize: macCredentials,
    extendInfo: {
      CFBundleDocumentTypes: [{
        CFBundleTypeName: "Markdown document",
        CFBundleTypeRole: "Editor",
        LSItemContentTypes: ["net.daringfireball.markdown", "public.plain-text"],
      }],
    },
  },
  makers: [
    {
      name: "@electron-forge/maker-squirrel",
      platforms: ["win32"],
      config: {
        name: "continuity_web",
        setupExe: "ContinuityWebSetup.exe",
        ...windowsCertificate,
      },
    },
    {
      name: "@electron-forge/maker-dmg",
      platforms: ["darwin"],
      config: { format: "ULFO" },
    },
    {
      name: "@electron-forge/maker-deb",
      platforms: ["linux"],
      config: {
        options: {
          name: "continuity-web",
          productName: "Continuity Web",
          genericName: "Markdown Editor",
          bin: "continuity-web",
          section: "editors",
          maintainer: "Continuity contributors",
          homepage: "https://github.com/continuity-editor/continuity",
          categories: ["Utility", "TextEditor"],
          mimeType: ["text/markdown", "text/plain"],
        },
      },
    },
    {
      name: "@electron-forge/maker-zip",
      platforms: ["win32", "darwin", "linux"],
    },
  ],
  publishers: [{
    name: "@electron-forge/publisher-github",
    config: {
      repository: { owner: "continuity-editor", name: "continuity" },
      prerelease: true,
      draft: true,
    },
  }],
  hooks: {
    postMake: async (_forgeConfig, makeResults) => signLinuxArtifacts(makeResults, process.env),
  },
};
