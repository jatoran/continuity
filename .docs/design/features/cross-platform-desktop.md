# Cross-platform desktop

Continuity Web is an Electron shell around the same packed
`@continuity-editor/editor` Web Component offered to embedders. It adds a
cross-platform application without replacing the native Win32 product or
creating another editor implementation.

## Shell decision

Electron 43.1.1 is pinned because Milestone 8 already measured and manually
validated the Chromium presentation used by the component. Its main/preload/
renderer boundary supports host-owned files, menus, updates, packaging, and
OS integration while keeping the renderer capability-free.

Tauri was not selected for this first shell. Its macOS and Linux webviews would
introduce unclaimed WebKit presentation surfaces, while its runtime uses Tokio;
silently adding that runtime to the root Rust workspace would violate repository
policy. Tauri remains possible only as an isolated workspace plus separately
funded WebKit/IME/accessibility evidence. Native macOS/Linux renderers remain
separate future adapters.

## Ownership

| Owner | State and responsibilities |
|---|---|
| Electron main process | single-instance ownership, window lifecycle, menus, settings, file dialogs, associations, external-change watcher, updates, durable store |
| Isolated preload | narrow named IPC methods; no general bridge |
| Renderer JavaScript agent | one Web Component, viewport, banners, ordered persistence queue |
| WASM engine | canonical text, revision, selections, undo, Markdown operations and projection |

The renderer has `nodeIntegration: false`, `contextIsolation: true`, and a
sandbox. A private `continuity://app/` origin serves the ASAR; navigation,
new windows, permissions, and untrusted IPC senders are rejected. The Content
Security Policy permits local scripts/WASM only. External links are validated
in main and opened by the OS.

## Storage and files

This shell uses host-managed persistence, not the native SQLite database. Each
accepted `continuity-change` is serialized in event-sequence order. Main writes
a complete versioned snapshot to a temporary file, syncs it, and rotates it
into one of two SHA-256-protected slots before acknowledging. Startup chooses
the newest valid durable sequence; corrupt/interrupted candidates produce a
non-modal recovery banner. A persistence failure freezes editing read-only and
offers copy-out rather than allowing unacknowledged revisions to accumulate.
Quit, window-close, and update-install all use the same renderer handshake: the
renderer drains its ordered persistence chain before main is allowed to exit.
Electron's single-instance lock ensures only one main process can write a given
application data root; later launches forward document paths to that owner.

A Node gate performs repeated synced 1 MiB snapshot writes and requires the
p99 acknowledgement to remain within the shared 400 ms durability ceiling.

Open imports one valid UTF-8 file up to 50 MiB and records its path/hash as
metadata. Export uses a synced temporary file plus recoverable previous-file
rotation. External changes produce Reload/Keep actions in a banner. The durable
store remains the recovery truth; associated files are exports. Uninstallers
retain the user-data store so removing the application does not delete writing.
On Windows, Squirrel registers a Continuity-owned Markdown ProgID, `.md` and
`.markdown` Open With entries, and Default Apps capabilities under HKCU. It
does not replace either extension's default handler, and uninstall removes only
those Continuity-owned values and keys.

## Accessibility and commands

The component remains one named multiline textbox; its visual projection stays
outside the accessibility tree. Electron main intercepts only the documented
editor-first browser conflicts while the editor is focused and sends portable
command ids to the component. Native selection/navigation and the Escape-then-
Tab exit contract remain intact. The Windows browser and Electron manual screen
reader rows passed on 2026-07-17.

## Budgets

The release gate applies on every claimed desktop OS:

| Measure | Budget | Windows evidence (2026-07-17) | macOS ARM64 evidence (2026-07-18) |
|---|---:|---:|---:|
| packaged startup | ≤4,000 ms | 191 / 180 ms | 1,532 / 279 ms |
| process-tree working set | ≤512 MiB | 294 / 285 MB | 415 / 406 MB |
| unpacked application | ≤450 MiB | 348.7 MiB | passed |
| each distributable | ≤200 MiB | ZIP 137.9 MiB; setup 133.7 MiB | DMG + ZIP passed |
| application ASAR | ≤8 MiB | 1.56 MiB | passed |

These are Electron-shell budgets, not native goals. The native Windows binary
retains its independent 9 MiB budget and existing latency/durability gates.

An isolated Debian amd64 run on 2026-07-17 additionally measured 239/228 ms
startup, 477/482 MiB process-tree working set, an 86.5 MiB deb, and a 119.6 MiB
ZIP. The unpacked and ASAR audits also passed their ceilings. It installed the
deb, verified `text/markdown` registration, recovered across two installed
launches, and purged the executable. Docker and hosted-runner functional
launches use `--no-sandbox` because those environments reject Chromium's
process namespace. The application configuration and shipped production
launches retain the normal Electron sandbox; automated Linux smoke does not
claim to validate that OS boundary.

Hosted macOS ARM64 CI on 2026-07-18 mounted the DMG, copied the application,
verified its Markdown document type, ran two installed launches through
recovery, and removed the copied application. The same hosted workflow proved
the Windows Squirrel and Ubuntu deb lifecycles, closing the three-platform
artifact exit gate.

## Distribution

Forge emits Squirrel + ZIP on Windows, DMG + ZIP on macOS, and deb + ZIP on
Linux. Release commands fail closed unless Windows signing, macOS signing and
notarization, or Linux GPG credentials are present for the current platform.
Linux release builds attach armored detached signatures to every artifact.
The deb package and executable are both named `continuity-web`; its desktop
entry declares `text/markdown` and `text/plain`.
Windows and macOS consume the configured GitHub release feed; Linux updates
through its package manager. File-type declarations support Open With, but
this preview does not take the native app's defaults.

`cargo xtask desktop-check` builds the exact packed Web Component, tests the
host store/files/settings/signing/registration, rejects production advisories,
audits ASAR contents, makes artifacts, and proves edit/export/restart/recovery,
single-writer handoff, and final-edit close durability against the packaged
executable. The CI matrix repeats it and exercises platform install/uninstall.
