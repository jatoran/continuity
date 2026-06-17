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
| `persist` | SQLite schema + edit log + snapshots + hot backup + recovery | `src/store.rs` + responsibility-scoped siblings under `src/store/` (`snapshots.rs`, `edits.rs`, `buffers.rs`, `trash.rs`, `undo_groups.rs`, `backup.rs`), `src/handle.rs`, `src/handle_timeline.rs`, `src/handle_metrics.rs`, `src/persist_loop.rs`, `src/codec.rs`, `src/checksum.rs`, `src/schema.rs`, `src/recover.rs`, `src/backup.rs`, `src/paths.rs` |

## Decoration + display projection

| Crate | Responsibility | Key files |
|---|---|---|
| `decorate` | Tree-sitter-md parse, markdown spans, headings, sections, autolinks, rainbow brackets, syntax highlights | `src/parser.rs`, `src/spans.rs`, `src/inline.rs`, `src/inline_text.rs`, `src/headings.rs`, `src/sections.rs`, `src/autolink.rs`, `src/rainbow.rs`, `src/syntax.rs`, `src/tables.rs`, `src/pool.rs`, `src/cache.rs`, `src/decorations.rs`, `src/language.rs` |
| `display_map` | Source ↔ display projection (hide / replace / fold / soft-wrap) | `src/builder.rs` + responsibility-scoped siblings under `src/builder/` (`segments.rs`, `segment_coalescing.rs`, `tests.rs`), `src/line.rs`, `src/segment.rs`, `src/style.rs`, `src/wrap.rs`, `src/id.rs` |
| `search` | Literal/regex find dispatcher, `grep-regex` branch, `memchr` literal branch, fuzzy scorer (FTS5 index dropped) | `src/dispatcher.rs`, `src/literal.rs`, `src/regex.rs`, `src/fuzzy.rs`, `src/index.rs` (legacy stub) |

## State machine + commanding

| Crate | Responsibility | Key files |
|---|---|---|
| `core` | Singleton editor state machine; sole writer of every `Buffer` | `src/state.rs`, `src/handle.rs`, `src/dispatch.rs`, `src/message.rs`, `src/selection_edit.rs`, `src/selection_coalesce.rs`, `src/edit_inline.rs`, `src/edit_lines.rs` (+ `src/edit_lines/toggle_bullet.rs`), `src/edit_line_text.rs` (+ `src/edit_line_text/trim.rs`), `src/edit_words.rs`, `src/edit_list.rs` (+ `src/edit_list/renumber.rs`), `src/edit_markdown*.rs` (+ `src/edit_markdown/emphasis.rs`, `src/edit_markdown/sections.rs`), `src/edit_pairs.rs`, `src/edit_indent_shift.rs`, `src/edit_planning.rs`, `src/undo.rs`, `src/policy.rs`, `src/clock.rs` |
| `command` | `Registry` + `Context` trait + `ContextPredicate` + command families | `src/registry.rs`, `src/context.rs`, `src/predicate.rs`, `src/id.rs`, `src/editor.rs`, `src/editor_extras.rs`, `src/selection.rs`, `src/view.rs`, `src/markdown.rs`, `src/clipboard.rs`, `src/file.rs`, `src/search.rs`, `src/tabs.rs`, `src/spell.rs`, `src/settings.rs` |
| `keymap` | TOML chord lookup, multi-chord sequence, conflict checker | `src/lib.rs`, `src/chord.rs`, `src/conflict.rs`, `assets/default.toml` |
| `input` | Win32 raw input → `KeyChord`, IME helpers | `src/lib.rs`, `src/chord.rs` |

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
| `ui` | HWND owner, pane tree, tab strip, overlays, keystroke dispatch, paint orchestration | `src/window.rs`, `src/window_commanding.rs`, `src/window_overlays.rs`, `src/window_paint.rs` + responsibility-scoped siblings under `src/window_paint/` (`frame_resolution.rs`, `cache_seed.rs`, `cold_deferred.rs`, `decorations.rs`, `dispatch.rs`, `payload.rs`, `view_options.rs`), `src/window_panes.rs`, `src/window_startup_open.rs`, `src/window_pane_layout_ops.rs` (apply_layout_shortcut / toggle_maximize / resize), `src/window_runtime.rs`, `src/window_view.rs`, `src/window_scroll.rs`, `src/window_view_context.rs`, `src/window_dismiss.rs`, `src/window_link_clipboard.rs`, `src/window_view_options.rs`, `src/window_settings_reload.rs`, `src/window_placement_persistence.rs`, `src/window_placement_apply.rs`, `src/window_clipboard.rs`, `src/clipboard_html.rs`, `src/html_to_markdown.rs`, `src/window_ime.rs`, `src/window_spell.rs`, `src/window_auto_pair.rs`, `src/window_pane_modes.rs`, `src/window_time_machine.rs`, `src/window_mouse_tabs.rs`, `src/window_mouse_splitter.rs`, `src/window_tab_drag.rs`, `src/window_tab_drag_ghost.rs`, `src/window_tab_drag_overlay.rs`, `src/window_tab_strip_scroll.rs`, `src/window_dispatch.rs` (+ `src/window_dispatch/panic_barrier.rs`), `src/window_view.rs` (+ `src/window_view/caret_reveal.rs`), `src/window_caret_anchor.rs` (+ `src/window_caret_anchor/resolve_build.rs`), `src/window_markdown_table_ops.rs` (+ `src/window_markdown_table_ops/paste_normalize.rs`), `src/pane_tree.rs`, `src/pane_state.rs`, `src/pane_layout.rs`, `src/pane_shortcuts.rs`, `src/selection.rs`, `src/selection_dispatch.rs`, `src/selection_vertical.rs`, `src/find_bar.rs`, `src/find_regex_help.rs`, `src/find_replace_plan.rs`, `src/find_scope.rs`, `src/window_find_replace.rs`, `src/window_find_scope.rs`, `src/window_find_target.rs`, `src/find_in_all.rs`, `src/search_minimap.rs`, `src/quick_open.rs`, `src/goto_overlay.rs`, `src/palette.rs`, `src/palette_rank.rs`, `src/palette_mode.rs`, `src/view_overlay.rs`, `src/overlay_render.rs`, `src/overlay_render_find.rs`, `src/overlay_render_palette.rs`, `src/file_io.rs`, `src/window_file.rs`, `src/jump_glow.rs`, `src/caret_tween.rs`, `src/smart_paste.rs`, `src/window_dismiss.rs`, `src/window_theme.rs`, `src/window_context_menu.rs`, `src/window_registry.rs` |

## App + test support

| Crate | Responsibility | Key files |
|---|---|---|
| `app` | Binary crate; wiring + `fn main`. Only crate allowed `anyhow` | `src/main.rs`, `src/main_initial_requests.rs`, `src/registry.rs` |
| `test_support` | Fixtures, golden buffers, `FakeClock`, proptest generators | `src/fixtures.rs`, `src/clock.rs`, `src/gen.rs`, `tests/canary.rs` (must always pass) |
| `xtask` | Workspace build / bench / release / conventions runner | `xtask/src/main.rs`, `xtask/src/conventions.rs`, `xtask/src/docs_gen.rs` |

## Ownership rules (quick reference)
- **`text` + `win`** — no internal deps; everyone else can depend on them.
- **`core`** — the only crate that owns mutable buffer state. Single-writer.
- **`ui`** — the only crate that touches HWNDs.
- **`app`** — the only crate with `fn main`. The only crate allowed `anyhow`.
- **Cross-layer `pub use`** — forbidden. Imports are explicit so the dependency graph stays legible.

