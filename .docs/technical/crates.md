# Crates

Workspace layout. Strict bottom-up layering. Lower crates know nothing of upper ones; no `pub use` re-exports across layer boundaries.

Generated companions: `.docs/generated/CRATES.md` for current counts/deps, `.docs/generated/modules/<crate>.md` for source files, `.docs/generated/api/<crate>.md` for public API, `.docs/generated/symbols/<crate>.md` for symbol localization.

## Leaves (no internal deps)

| Crate | Responsibility | Key files |
|---|---|---|
| `text` | `Position`, `Range`, `Selection`, `EditOp`, selection helpers | `src/position.rs`, `src/range.rs`, `src/selection.rs`, `src/edit.rs`, `src/select.rs` |
| `win` | Win32 wrappers: window class, HWND, DPI, virtual desktops, COM apartment, single-instance, clipboard | `src/window.rs`, `src/dpi.rs`, `src/virtual_desktop.rs`, `src/com.rs`, `src/single_instance.rs`, `src/clipboard.rs` (text + CF_HTML read/write), `src/clipboard_image.rs`, `src/ime.rs`, `src/dwm.rs`, `src/icon.rs`, `src/monitor.rs` |

## Buffer + persistence

| Crate | Responsibility | Key files |
|---|---|---|
| `buffer` | `Buffer` aggregate, `Revision`, `Selection`, undo tree, snapshot, incremental checksum | `src/buffer.rs`, `src/buffer/`, `src/checksum.rs`, `src/undo.rs`, `src/revision.rs`, `src/snapshot.rs`, `src/file.rs`, `src/id.rs` |
| `persist` | SQLite schema + edit log + snapshots + hot backup + recovery | `src/store.rs` + responsibility-scoped siblings under `src/store/` (`snapshots.rs`, `edits.rs`, `buffers.rs`, `trash.rs`, `undo_groups.rs`, `backup.rs`), `src/handle.rs`, `src/handle_timeline.rs`, `src/persist_loop.rs`, `src/codec.rs`, `src/checksum.rs`, `src/schema.rs`, `src/recover.rs`, `src/backup.rs`, `src/paths.rs` |

## Decoration + display projection

| Crate | Responsibility | Key files |
|---|---|---|
| `decorate` | Tree-sitter-md parse, markdown spans, headings, sections, autolinks, rainbow brackets, syntax highlights | `src/parser.rs`, `src/spans.rs`, `src/inline.rs`, `src/inline_text.rs`, `src/headings.rs`, `src/sections.rs`, `src/autolink.rs`, `src/rainbow.rs`, `src/syntax.rs`, `src/tables.rs`, `src/pool.rs`, `src/cache.rs`, `src/decorations.rs`, `src/language.rs` |
| `display_map` | Source ↔ display projection (hide / replace / fold / soft-wrap) | `src/builder.rs` + responsibility-scoped siblings under `src/builder/` (`segments.rs`, `segment_coalescing.rs`, `tests.rs`), `src/line.rs`, `src/segment.rs`, `src/style.rs`, `src/wrap.rs`, `src/id.rs` |
| `search` | Literal/regex find dispatcher, `grep-regex` branch, `memchr` literal branch, fuzzy scorer (FTS5 index dropped) | `src/dispatcher.rs`, `src/literal.rs`, `src/regex.rs`, `src/fuzzy.rs`, `src/index.rs` (legacy stub) |

## State machine + commanding

| Crate | Responsibility | Key files |
|---|---|---|
| `engine` | Synchronous storage-neutral buffer state, planners, undo/coalescing, deltas, `ChangeBatch`, events | `src/engine.rs`, `src/change.rs`, `src/undo.rs`, `src/state.rs`, `src/selection_edit.rs`, `src/edit_*.rs`, `src/delta_history.rs` |
| `host` | Platform-neutral intents, typed editor operations, post-dispatch event batches, checked UTF-16 boundaries, optional ephemeral runtime | `src/intent.rs`, `src/operation.rs`, `src/event.rs`, `src/runtime.rs`, `src/utf16.rs` |
| `wasm` | Thin synchronous `wasm-bindgen` transport over engine, decoration, and display projection | `src/editor.rs`, `src/report.rs`, `src/projection.rs`, `tests/parity_native.rs` |
| `c_api` | Versioned C ABI, panic boundary, allocator/thread contract, checked public header | `src/api.rs`, `src/handle.rs`, `src/types.rs`, `include/continuity_engine.h` |
| `continuity-python` | PyO3 `abi3` headless Python facade over one synchronous engine document | `bindings/python/src/lib.rs`, `bindings/python/pyproject.toml` |
| `core` | Native threaded `Engine` host and SQLite/snapshot adapter | `src/handle.rs`, `src/handle/core_loop.rs`, `src/dispatch.rs`, `src/persistence_bridge.rs`, `src/message.rs`, `src/policy.rs`, `src/clock.rs` |
| `command` | Desktop `Registry`/`Context` plus context-free command-to-`EditorOperation` resolution | `src/portable_operation.rs`, `src/registry.rs`, `src/context.rs`, `src/predicate.rs`, command-family modules |
| `keymap` | TOML chord lookup, multi-chord sequence, conflict checker | `src/lib.rs`, `src/chord.rs`, `src/conflict.rs`, `assets/default.toml` |
| `input` | Platform-neutral key-chord grammar and modifiers | `src/lib.rs`, `src/chord.rs` |

## Theme + config

| Crate | Responsibility | Key files |
|---|---|---|
| `theme` | TOML themes, required key set, hot reload | `src/theme.rs`, `src/color.rs`, `src/keys.rs`, `src/mode.rs`, `src/assets.rs`, `assets/{deep_minimal,paper}.toml` |
| `config` | `Settings`, validation, watcher, autocorrect rules | `src/settings.rs`, `src/validate.rs`, `src/watcher.rs`, `src/mode.rs`, `src/autocorrect.rs`, `src/error.rs` |

## Layout + render

| Crate | Responsibility | Key files |
|---|---|---|
| `layout` | DirectWrite `IDWriteTextLayout` cache, hit testing, soft-wrap measurement | `src/cache.rs`, `src/view_state.rs`, `src/lib.rs` |
| `render` | Direct2D draw, swap chain, atlas-free pipeline | `src/renderer.rs`, `src/renderer_draw_main.rs` (+ `src/renderer_draw_main/minimap_pass.rs`), `src/chrome.rs`, `src/chrome_caret.rs`, `src/chrome_post.rs`, `src/wrap_paint.rs`, `src/decoration_paint.rs`, `src/pane_body.rs`, `src/pane_chrome.rs` (+ `src/pane_chrome_layout.rs`, `src/pane_chrome_chevron.rs`), `src/spell.rs`, `src/overlay.rs`, `src/overlay_scrollbar.rs`, `src/status_bar.rs`, `src/display_projection.rs`, `src/scrollbar.rs`, `src/text_helpers.rs`, `src/text_metrics.rs`, `src/params.rs` |

## UI

| Crate | Responsibility | Key files |
|---|---|---|
| `ui` | HWND owner, desktop message pump, evolving reusable editor surface, pane/tab shell, overlays, input, and paint orchestration | `src/desktop_shell.rs`, `src/editor_surface.rs`, `src/editor_surface/projection.rs`, `src/window.rs`, `src/window_commanding.rs`, `src/window_paint.rs` and siblings, `src/window_ime.rs`, `src/window_clipboard.rs`, `src/window_dispatch.rs`, `src/pane_tree.rs`, `src/pane_state.rs` |

## App + test support

| Crate | Responsibility | Key files |
|---|---|---|
| `app` | Binary crate; wiring + `fn main`. Only crate allowed `anyhow` | `src/main.rs`, `src/main_initial_requests.rs`, `src/registry.rs` |
| `test_support` | Fixtures, golden buffers, `FakeClock`, proptest generators | `src/fixtures.rs`, `src/clock.rs`, `src/gen.rs`, `tests/canary.rs` (must always pass) |
| `test_fixtures` | Dependency-free semantic parity corpus | `src/parity_corpus.rs` |
| `xtask` | Workspace build / bench / release / conventions runner | `xtask/src/main.rs`, `xtask/src/conventions.rs`, `xtask/src/docs_gen.rs` |

The stable JavaScript/TypeScript API is not the generated `wasm` crate output;
it lives in `packages/editor/index.js` and `index.d.ts`. Package and clean
consumer validation are owned by `xtask/src/wasm.rs`.

## Ownership rules (quick reference)
- **`text` + `win`** — no internal deps; everyone else can depend on them.
- **`engine`** — owns mutable buffer state; one caller-selected writer per instance.
- **`core`** — native Windows engine owner and durability adapter.
- **`ui`** — the only crate that touches HWNDs.
- **`app`** — the only crate with `fn main`. The only crate allowed `anyhow`.
- **Cross-layer `pub use`** — forbidden. Imports are explicit so the dependency graph stays legible.

