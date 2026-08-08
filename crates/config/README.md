# config

`settings.toml` loader plus a `notify`-backed file watcher for live reload,
and the data-only `.continuity/vault.toml` contract. `vault.rs` validates
folder-scoped autosave, ignore/sort/display rules, path colors, and theme
overrides; filesystem discovery and watching remain in `ui`'s file-I/O worker.
Newly initialized markers use `solarized_dark`, Solarized file/folder label
colors, and `.trash` as the sole active ignore; commented examples document
wildcards, re-includes, path styles, and token overrides.

`vault_workspace.rs` defines the separate `.continuity/workspace.toml`
contract for portable file-tree width, visibility, and expanded-directory
state. It validates only data; the UI file-I/O worker owns reads and writes.

Layer: glue. No internal deps.
