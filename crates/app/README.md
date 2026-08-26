# app

The binary crate. The only place with `fn main()` and the only place
`anyhow` is allowed. Wires every other crate together.

Produces the `continuity` executable.

Startup resolves runtime paths before worker threads start. Normal launches
use `%APPDATA%\continuity`; `--portable` or a `data\` directory beside the
executable routes settings, keymap, themes, database, and tutorial state to
the executable folder instead.

A bare second launch activates a Continuity window on the current virtual
desktop or asks the registry to spawn a fresh blank window there. Persisted
desktop placement remains exclusive to crash/session restoration.

External file activations are collected for a 120 ms quiet window with a 350 ms cap.
The registry resolves every file to its canonical buffer and spawns one new window with all selected files as tabs.
This covers Explorer's per-file legacy verb launches as well as a direct multi-path command line.

`--vault <root>` is stable shortcut/automation intent. It routes through the
single-instance hub when necessary, reuses a matching vault only on the
current virtual desktop, and otherwise opens a fresh local window.

The registry also routes file-tree Preview, NewTab, and dropped-file opens back to the source window and target pane.
NewWindow opens spawn a top-level window and carry the active vault root so Shift+click preserves the file sidebar.
Each live window also registers its HWND with the registry, allowing latency-sensitive control messages to wake the UI receiver immediately instead of waiting for its poll.
