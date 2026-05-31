# app

The binary crate. The only place with `fn main()` and the only place
`anyhow` is allowed. Wires every other crate together.

Produces the `continuity` executable.

Startup resolves runtime paths before worker threads start. Normal launches
use `%APPDATA%\continuity`; `--portable` or a `data\` directory beside the
executable routes settings, keymap, themes, database, and tutorial state to
the executable folder instead.
