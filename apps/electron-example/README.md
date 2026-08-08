# Continuity Electron host example

This deliberately small host imports the packed `@continuity-editor/editor`
package in its renderer. The main process owns an atomic JSON persistence file;
the context-isolated preload exposes only load, WASM-byte, and ordered persist
IPC methods. No Electron API enters the editor package.

The load path restores both text and engine revision. The main process assigns
its own durable monotonic sequence independently of each renderer session's
local event sequence. The smoke gate launches the host twice against the same
data. Each launch executes a main-process Ctrl+E command plus a normal edit,
requiring revision/sequence `2 -> 4` continuity.

For local development, build the WASM package before installing dependencies.
The gated smoke fixture is assembled from the npm tarball by
`cargo xtask browser-check`; it does not consume the monorepo package path.

The renderer uses only browser APIs and the packed component. A strict content
security policy permits local scripts and WebAssembly compilation but no
network connection. The context-isolated preload is the complete trust
boundary: the renderer cannot import Electron or Node APIs.

After `cargo xtask browser-check`, the clean packed fixture remains under
`target/wasm-sdk/electron-consumer`. Run `npm start` there for an interactive
host check. Its JSON document lives in Electron's user-data directory and is
not the native Continuity SQLite database, so it cannot affect an installed
native application.

The interactive host includes focus, read-only, and destroy/recreate controls
plus a polite live status region. They exercise the complete manual
screen-reader acceptance path: named multiline editing, spoken host persistence
updates, read-only state, focus escape, and teardown without a ghost editor.
Set `CONTINUITY_ELECTRON_USER_DATA` to a throwaway directory when an explicitly
disposable document is preferred.

Editable mode treats Tab/Shift+Tab as engine-backed line indent/outdent. Press
Escape before Tab or Shift+Tab to traverse to the adjacent host control;
read-only mode traverses directly. The editor's accessible description exposes
the same instruction to screen readers. Engine-backed edits keep the active
caret inside the textarea viewport in both directions and update the Markdown
projection in the same turn; the packed Electron smoke probes this with a long
document before exercising persistence. It also injects Ctrl/Cmd+Alt+Down
through `webContents`, requiring the engine snapshot and visible overlay to
agree on the exact active and secondary caret positions.

The renderer selects `editor-first`. While the semantic textarea owns focus,
the main process uses `webContents` `before-input-event` for Chromium/menu
conflicts (Ctrl+E/K/R/J/U and their registered Shift variants), cancels the
accelerator, and sends only the portable command ID through the context-isolated
preload. Non-conflicting chords stay in the renderer; focus outside the editor
disables interception. The smoke injects a real Ctrl+E through this path.
