# Continuity Web desktop

Electron host for the packed `@continuity-editor/editor` Web Component. It is
a separate preview product from the native Win32 application: it has a distinct
application id, executable, installer root, and user-data directory.

## Ownership

The renderer owns one Web Component and no Node.js capability. The main process
owns windows, menus, file dialogs, UTF-8 import/export, external-file watching,
settings, updates, and the durable document store. `preload.cjs` exposes only
named, validated IPC operations through context isolation.

Accepted editor changes are serialized and acknowledged only after a full
snapshot is synced and rotated into one of two integrity-checked JSON slots.
An interrupted slot is ignored on restart and reported in a banner. Files are
imports/exports; the host-managed store remains the recovery source of truth.

Default user-data roots are `%APPDATA%/Continuity Web` on Windows,
`~/Library/Application Support/Continuity Web` on macOS, and
`~/.config/Continuity Web` on Linux. Uninstallers remove application binaries
and shortcuts but retain user documents by design.

## Development

Run the repository-owned gate from the workspace root:

```text
cargo xtask desktop-check
```

It builds the WASM editor, installs the exact npm tarball, tests and audits the
host, creates platform distributables, audits the ASAR, and launches the
packaged application against disposable data for two-launch recovery,
single-instance handoff, and final-edit close durability. Direct `npm install`
alone links the source package, which intentionally lacks generated WASM; use
the xtask gate for release-faithful testing.

For renderer iteration after `cargo xtask wasm-package`, install the emitted
tarball with `npm install --no-save <tarball>`, then run `npm start`.

Release signing is opt-in for development builds and fails closed in
`npm run make:release` and `npm run publish`:

- Windows: `WINDOWS_CERTIFICATE_FILE` and `WINDOWS_CERTIFICATE_PASSWORD`.
- macOS: `APPLE_ID`, `APPLE_APP_SPECIFIC_PASSWORD`, and `APPLE_TEAM_ID`.
- Linux: `CONTINUITY_LINUX_GPG_KEY` creates armored detached signatures for
  every Forge artifact.

`npm run publish` additionally requires `GITHUB_TOKEN`. Pull-request CI creates
unsigned artifacts for lifecycle testing; it never claims release readiness.

Forge produces Squirrel + ZIP on Windows, DMG + ZIP on macOS, and deb + ZIP on
Linux. Windows/macOS use Electron's updater against the GitHub release feed;
Linux updates through its package manager.

Only one process owns a user-data root. Later launches forward Markdown paths
to the primary process. Windows registers Open With and Default Apps
capabilities for `.md`/`.markdown` without replacing the user's default editor.
