# Import boundaries

Strict bottom-up dependency layering. Lower layers must not know about upper ones. No `pub use` re-exports across layer boundaries — explicit imports keep the dependency graph legible.

## Layer graph

```
text · win                                       # leaves — no internal deps
buffer ← text                                    # Buffer aggregate
persist ← buffer                                 # SQLite, edits, snapshots, backup
decorate ← buffer                                # tree-sitter, markdown spans
search ← buffer                                  # literal/regex find + fuzzy scoring
display_map ← buffer · decorate                  # source ↔ display projection
core ← buffer · persist · text                   # SOLE writer of buffer state
command ← core · text · buffer                   # registry + Context + predicates
keymap ← command · input                         # TOML chord lookup
theme · config                                   # TOML loaders + watcher
layout ← win                                     # DirectWrite layout cache
render ← layout · win · display_map              # D3D11 + DXGI + D2D + DWrite
ui ← render · command · keymap · core · display_map · …
app ← ui · core · persist · command · keymap     # only fn main; only `anyhow`
test_support ← buffer · text · persist           # fixtures, FakeClock, gens
xtask                                            # workspace tasks
```

## Rules

### CORRECT examples

```rs
// crates/core/src/dispatch.rs — `core` may use `buffer`, `text`, `persist`
use continuity_buffer::{Buffer, BufferId, Revision};
use continuity_text::Selection;
use continuity_persist::PersistClient;

// crates/ui/src/window_search.rs — `ui` may use `core`, `command`, `search`, `decorate`
use continuity_core::EditorHandle;
use continuity_search::find_match_ranges_dispatch;
use continuity_decorate::HeadingEntry;
```

### WRONG examples

```rs
// WRONG: `buffer` may not depend on `core`
// (buffer is *below* core)
use continuity_core::EditorHandle;     // ❌ in any file under crates/buffer/

// WRONG: `decorate` may not touch `display_map`
// (display_map depends on decorate, not the other way around)
use continuity_display_map::DisplayMap; // ❌ in any file under crates/decorate/

// WRONG: `core` may not touch HWND types
// (ui owns HWND; core is headless)
use windows::Win32::Foundation::HWND;   // ❌ in any file under crates/core/

// WRONG: cross-layer pub use
// (no re-exports of upper-layer types from lower layers)
pub use continuity_core::EditorHandle;  // ❌ in any non-`app` crate
```

## Per-crate dependency manifests

| Crate | Allowed deps |
|---|---|
| `text` | none |
| `win` | none |
| `buffer` | `text` |
| `persist` | `buffer`, `text` |
| `decorate` | `buffer`, `text` |
| `search` | `buffer`, `text` |
| `display_map` | `buffer`, `decorate`, `text` |
| `core` | `buffer`, `persist`, `text` |
| `command` | `core`, `buffer`, `text` |
| `keymap` | `command`, `input` |
| `input` | `win` |
| `theme` | none (TOML only) |
| `config` | none (TOML only) |
| `layout` | `win` |
| `render` | `layout`, `win`, `display_map`, `decorate`, `text`, `theme` |
| `ui` | every crate below it |
| `app` | `ui`, `core`, `persist`, `command`, `keymap`, `config`, `theme` |
| `test_support` | `buffer`, `text`, `persist` |

## Single-owner rules

- **Mutable `Buffer` state**: `core` only.
- **HWND**: `ui` only.
- **`fn main`**: `app` only.
- **`anyhow`**: `app` and `xtask` only. Every other crate has its own `thiserror::Error` enum in `src/error.rs`.

## Forbidden patterns

```rs
// WRONG: tokio / async-std anywhere
use tokio::sync::Mutex;            // ❌ no async runtime in this project

// WRONG: `async fn` anywhere
async fn write_edit(…)  { … }      // ❌ sync-with-threads only

// WRONG: glob imports outside #[cfg(test)]
use foo::*;                        // ❌ explicit imports only

// WRONG: bare TODO
// TODO: fix later                  ❌
// TODO(#247): clamp on overflow    ✓ (issue-tagged)
```

## Enforcement

`cargo xtask conventions` checks: file length (≤600, unconditional), no-mod-rs (foo.rs + foo/ layout), no-unwrap-panic in non-test code, anyhow scope (app + xtask only), no glob imports, bare TODO, no `async fn`, no `tokio` in `Cargo.lock`.

Run before push:
```
cargo xtask ci && cargo xtask conventions
```

Both must be green. Pre-push git hook (`.githooks/pre-push`) runs them automatically when installed via `cargo xtask install-hooks`.
