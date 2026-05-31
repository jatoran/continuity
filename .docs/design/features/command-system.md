# Command system

Every editor action is a `CommandId` + `ContextPredicate` + handler closure registered in `command::Registry`. The keymap, command palette, slash palette, and tests all dispatch through this single funnel. Handlers run against a `&mut dyn Context`; the production implementor is `ui::Window`.

## What it is
- Every editor action is a `Command`. A `Registry` maps `CommandId(&'static str) → (ContextPredicate, Handler)`. Dispatch resolves the handler by id and predicate, invokes it against a `&mut dyn Context`. The keymap, palette, slash commands, and tests all dispatch the same way.

## Key concepts
- **`CommandId(pub &'static str)`** — stable identifier (e.g. `"editor.indent"`). Cheap to compare; serialized as the bare string in keymap TOML.
- **`Handler = Arc<dyn Fn(&Value, &mut dyn Context) -> Result<(), Error> + Send + Sync>`** — args land as JSON, body operates against the context. Cheap to clone (Arc).
- **`Registry`** — stores `(CommandId → (ContextPredicate, Handler))` plus the palette-safe id set.
- **`Context`** — narrow trait every handler operates against. Production impl: `ui::Window`. Headless tests stub a tiny subset.
- **`FileContext`** — opt-in file/folder command surface returned by `Context::file_context()`. Keeps native dialogs and file-tree toggles out of generic test contexts.
- **`ContextPredicate`** — `expr := atom (&& atom)*`; atoms are `editor.focused`, `find_bar.visible`, `selection.is_caret`, `shift.held`, `language`, etc. No `||`, no parens. Documented in [`interfaces.md`](../interfaces.md).
- **Palette-safe set (Phase A7)** — opt-in flag; slash commands (H5) and future restricted palette modes filter on it.

## Data model

```rs
struct CommandId(pub &'static str);

type Handler = Arc<dyn Fn(&Value, &mut dyn Context) -> Result<(), Error> + Send + Sync>;

impl Registry {
    fn register             (&mut self, id, when, handler);
    fn register_palette_safe(&mut self, id, when, handler);
    fn mark_palette_safe    (&mut self, id);
    fn is_palette_safe      (&self, id: &str) -> bool;
    fn palette_safe_ids     (&self) -> Vec<&'static str>;
    fn dispatch             (&self, id, args: &Value, ctx: &mut dyn Context) -> Result<(), Error>;
    fn handler_for_name     (&self, name: &str, ctx: &dyn Context) -> Result<Handler, Error>;
}
```

`handler_for_name` is how the keymap looks up by string name (the keymap loads bindings as `(chord, name)`).

## Predicates

The grammar is intentionally tiny. New atoms land via `Context::flag` (`bool`) or `Context::lookup` (`Option<&str>`).

```
editor.focused              flag, true when the editor surface owns focus
find_bar.visible            flag, true while `Overlays::Find` is active
selection.is_caret          flag, all selections are collapsed carets
shift.held                  flag, modifier currently down
language                    lookup, "plain" | "markdown" | language tag
```

Predicate-gated bindings example:

```toml
[[binding]]
keys = ["tab"]
command = "editor.indent"
# when = "editor.focused"     # implicit; the keymap loader adds it if omitted
```

## Operations
- **Register** a command via one of the `register_*_commands(&mut Registry)` functions per family (`register_editor_primitives`, `register_selection_commands`, `register_view_commands`, `register_markdown_commands`, `register_clipboard_commands`, …). Wiring lives in `app::registry::build_registry`.
- **Dispatch** via `Registry::dispatch(id, args, ctx)` from the keymap on `WM_KEYDOWN`, the palette on confirm, or directly from tests.
- **Reject** unknown commands or failed predicates with `Error::UnknownCommand` / `Error::UnmetPredicate`. UI logs but keeps running.
- **File/folder commands** route through `ctx.file_context()` first. Supported ids include `file.open`, `file.open_folder`, `file.save`, `file.save_as`, and `view.toggle_file_tree`.

## Extension recipe (add a new command)
1. Define `pub const NEW_THING: CommandId = CommandId("editor.new_thing");` in the relevant `crates/command/src/<family>.rs`.
2. Register in the family's `register_*` function with the appropriate predicate.
3. Bind in `crates/keymap/assets/default.toml`.
4. Implement on `Context` (default returns `UnsupportedContext("new_thing")`).
5. Provide the production impl in `ui::window_commanding.rs` (or domain-specific window module).
6. Re-export the `CommandId` from `crates/command/src/lib.rs` if external crates need to reference it.
7. Add `#[test]` covering the dispatch round-trip in `crates/command/src/<family>.rs::tests`.

## Configuration
- `keymap/assets/default.toml` — default bindings.
- `%APPDATA%\continuity\keymap.toml` — user overrides. Layered on top of defaults; conflict checker reports collisions.

## Key files
- registry: `crates/command/src/registry.rs`
- id newtype: `crates/command/src/id.rs`
- predicates: `crates/command/src/predicate.rs`
- context trait: `crates/command/src/context.rs`
- file command trait: `crates/command/src/file_context.rs`
- families:
  - editor primitives: `crates/command/src/editor.rs`
  - editor extras: `crates/command/src/editor_extras.rs`
  - selections: `crates/command/src/selection.rs`
  - view: `crates/command/src/view.rs`
  - markdown: `crates/command/src/markdown.rs`
  - clipboard: `crates/command/src/clipboard.rs`
  - file: `crates/command/src/file.rs`
  - search: `crates/command/src/search.rs`
  - tabs / panes / windows: `crates/command/src/tabs.rs`
  - spell: `crates/command/src/spell.rs`
  - settings: `crates/command/src/settings.rs`
- wiring: `crates/app/src/registry.rs::build_registry`
- Window impl: `crates/ui/src/window_commanding.rs`, `window_overlays.rs`, `window_view_context.rs`, `window_view_options.rs`, etc.

## Relates to
- [Keymap](keymap.md) — chord → command id mapping.
- [Interfaces](../interfaces.md) — `Context` trait surface, predicate grammar.
- [Overlays](overlays.md) — the palette dispatches commands by id; the slash palette filters on `palette_safe`.
- [Selections + edits](selection-edits.md) — most editor commands ultimately call `ctx.apply_selection_edit(SelectionEdit::X)`.
