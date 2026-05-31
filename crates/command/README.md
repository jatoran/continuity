# command

Command registry, context predicates, dispatch. The single funnel through
which the keymap, command palette, and tests invoke editor behavior.

Layer: middle. Depends on `buffer`, `core`.

Owns one embedded asset: `assets/tutorial.md`, surfaced as
`TUTORIAL_MD` and consumed by `help.tutorial` (see `src/help.rs`). The
asset is auto-generated — run `cargo xtask gen-tutorial` to regenerate
after touching feature docs or the default keymap; `cargo xtask
conventions` enforces it stays in sync.

`src/search.rs` owns the command ids for find-bar navigation, replace-one,
replace-all, mode toggles, scope toggle, and matches-to-cursors. Toggle
commands are gated by the `find_bar.visible` predicate and route through
`FindContext::find_toggle(mode)` so the command trait surface stays narrow.

`src/file_context.rs` owns the optional file/folder command surface. File
commands (`file.open`, `file.open_folder`, `file.save`, external-change
actions) and the `view.toggle_file_tree` view command first ask
`Context::file_context()` so lightweight contexts do not need native dialog
or file-tree methods.
