# Desktop Web implementation

## Source map

- `apps/desktop-web/main.mjs` — Electron lifecycle and validated IPC.
- `preload.cjs` — context-isolated capability bridge.
- `renderer.mjs` — Web Component composition and ordered durability queue.
- `src/storage.mjs` — two-slot synced snapshot store and recovery.
- `src/files.mjs` — bounded UTF-8 import and recoverable atomic export.
- `apps/desktop-web/src/windows_registration.mjs` — HKCU Open With/capability
  registration.
- `apps/desktop-web/src/release_signing.mjs` and
  `apps/desktop-web/scripts/release.mjs` — fail-closed release entry points and
  platform signing policy.
- `src/menu.mjs`, `settings.mjs`, `updater.mjs`, `protocol.mjs` — host adapters.
- `forge.config.mjs` — platform makers, signing, document types, publisher.
- `xtask/src/desktop.rs` — clean package, artifact audit, smoke, and budgets.

## Validation

```text
cargo xtask desktop-check
```

The command first builds the release WASM/npm artifact. It runs `npm ci`, then
replaces the source link with that exact tarball so missing generated WASM can
never be masked by a developer tree. It runs the Node tests and a production-
dependency advisory audit, makes the host-platform artifacts in a unique
`target/desktop-web/out-*` directory, checks required formats and size budgets,
and inventories `app.asar`.

The packaged executable launches twice with a disposable user-data root. Run 1
types through the semantic textarea, waits for the durable acknowledgement,
exports exact text, and stages a truncated next slot. Run 2 must report that
recovery, restore run 1, type again, export again, and continue revision 1→2
and durable sequence 2→4. Both runs also assert the named multiline semantic
textbox attributes.

A third packaged probe edits and immediately requests quit; main must delay
exit until the renderer's pending snapshot is synced, then a fresh store read
must contain that final text. A separate two-process probe proves the Electron
single-instance lock forwards a Markdown path to the primary process and does
not create a second persistence owner. The harness waits for that primary to
exit after its handoff marker appears before parsing the JSON payload, because
the file becomes visible before the asynchronous write is complete. Node tests
cover storage corruption and interruption, settings normalization, external
file watching, Windows registry command ownership, release credential
rejection, Linux detached signing, and the representative 1 MiB/400 ms
durability budget.

`.github/workflows/ci.yml` repeats this on Windows, macOS, and Linux, then
exercises the native installer/container lifecycle and removes it. Signing is
not performed in pull-request CI; release credentials are separate secrets.
The Windows lifecycle explicitly exits successfully after its expected-negative
registry queries, preventing the final proof-of-removal `reg.exe` status from
overriding the completed assertions. Squirrel removes registration before its
spawned updater necessarily finishes deleting the executable, so the lifecycle
polls for both conditions within the same bounded 60-second uninstall window.
The macOS lane probes the runner image's installed Homebrew LLVM formulas,
selects the first `clang`/`llvm-ar` pair that compiles a WASM object, and only
then builds tree-sitter. This accommodates `macos-latest` moving between images
with LLVM 18 and LLVM 20; Apple Clang does not ship the
`wasm32-unknown-unknown` backend.
The Linux lane installs `fakeroot`, `libnotify4`, Xvfb, D-Bus session tools,
and `zip` explicitly. Packaged smoke launches run inside a fresh
`dbus-run-session` and Xvfb display so startup measures the application against
valid desktop services instead of waiting on invalid hosted-runner bus
addresses. Its deb maker pins both the Debian package and executable name to
`continuity-web`; lifecycle validation selects the exact
`/usr/bin/continuity-web` path before checking MIME registration, two-launch
recovery, and purge.
Every automated Linux smoke runs the executable with `--no-sandbox` because
the unpacked Chromium helper lacks installed ownership and the hosted runner's
process namespace rejects the installed helper. The flag must be present at
process startup; appending it from the smoke bootstrap is too late for the
initial Chromium zygote. The installed-deb lifecycle still proves package
installation, MIME registration, edit/export/recovery, and purge. Production
launches omit the flag and retain the configured sandboxed renderer.

## Persistence order

Correct:

```text
continuity-change N → queued main IPC → sync temporary snapshot → rotate slot
→ acknowledge N → allow N+1 persistence
```

Incorrect:

```text
continuity-change N → show “saved” → write later or allow N+1 to overtake it
```

Renderer close waits the persistence chain before acknowledging main's close
request; File → Quit and update installation use that same path. Main validates
every snapshot, sender origin, URL scheme, settings
shape, external-change token, and smoke-only operation. It alone mutates the
durable store and file association. The renderer alone mutates editor and
presentation state on its JavaScript agent.

## Release notes

`CONTINUITY_DESKTOP_OUT_DIR` selects a fresh Forge output root and prevents
Windows file locks from poisoning the next build. Packager ignores every
`out*` tree, tests, lockfile, and development README; the ASAR gate requires the
packed editor's generated JavaScript and WASM and rejects development entries.

Windows Squirrel uninstall removes the runnable app, package cache,
registration, and shortcuts; its updater may leave a `.dead` self-removal
tombstone until Windows permits deletion. CI treats any runnable executable or
shortcut as failure and deletes only that non-user-data tombstone afterward.
User-data retention is intentional recovery behavior on every platform.

`npm run make:release` and `npm run publish` set the release-build contract.
They reject missing platform credentials before Forge starts; publication also
requires `GITHUB_TOKEN`. Windows uses its certificate file/password, macOS uses
Apple signing plus notarization credentials, and Linux uses
`CONTINUITY_LINUX_GPG_KEY` to append `.asc` signatures in `postMake`.
