# Conventions

Machine-checkable rules live in `xtask/src/conventions.rs`. This doc summarizes them with rationale + examples. Long-form rationale lives in `.docs/development/development_conventions.md`.

## File hygiene

### 600-line file cap

Every file must stay under 600 lines. The rule is unconditional — there is no per-file exemption mechanism. When a file crosses the cap, split it into responsibility-scoped siblings (`foo.rs` + `foo/<helper>.rs`, no `mod.rs`).

```rs
// CORRECT: file growing past 600 → split into sibling.
// before:  crates/foo/src/widget.rs (650 lines)
// after:   crates/foo/src/widget.rs       (320 lines, public API + types)
//          crates/foo/src/widget/build.rs (330 lines, construction logic)
```

### No `mod.rs`

Use `foo.rs` + `foo/` directory layout. Never `foo/mod.rs`.

```
CORRECT:                       WRONG:
crates/foo/src/widget.rs       crates/foo/src/widget/mod.rs
crates/foo/src/widget/         crates/foo/src/widget/parts.rs
crates/foo/src/widget/parts.rs
```

### One concept per module
`buffer/src/undo.rs` holds `UndoTree` and nothing else. If a module starts importing types from a sibling for a tightly-coupled subsystem, the two probably belong in the same file or under the same `foo/` directory.

### Filenames describe responsibility, not history (`no-phase-prefixed-filename`)

Never name a file by the roadmap phase it came from. `window_phase11.rs`, `phase7_undo.rs`, `phase_h.rs` all violate the rule. Phase numbers stop being a coordinate the moment the next phase lands; "what does this file do?" needs to be answerable from the path.

The check at `xtask/src/conventions.rs::check_no_phase_prefixed_filename` rejects basenames matching phase-coordinate shapes under `crates/*/src/**` and `crates/*/tests/**`: `phase[0-9_]…\.rs`, `window_phase[0-9_]…\.rs`, and contained coordinates like `*_phase_i.rs` / `*_phase10.rs`. The boundary keeps unrelated names like `phaser.rs` and `moon_phase_calculator.rs` from false-positive.

```
WRONG                              CORRECT
window_phase11.rs                  window_view_options.rs
window_phase16_5_pairs.rs          window_auto_pair.rs
phase8_search.rs                   search_integration.rs
view_phase_h.rs                    view_modes.rs
```

The roadmap is the historical record; the codebase is the current state.

### Identifiers describe responsibility, not phase (`no-phase-prefixed-{function,type,const}`)

The same rule extends to identifier names inside the files. Function (including `#[test]`), type (`struct` / `enum` / `trait` / `type`), and constant (`const` / `static`) names are rejected when they encode a roadmap phase coordinate:

- starts with `phase` (any case) followed by digit, underscore, or end — e.g. `phase6_foo`, `PhaseHState`, `PHASE_F3_TAG`.
- starts with `<single-letter><digits>_` — e.g. `h6_tab_overlay`, `i2_view_metrics`, `H6_CONST`.
- contains `_phase` (or `_PHASE`) followed by digit, end, or `_<single-letter>` where the letter is itself terminal / digit / underscore-bounded — e.g. `apply_phase9_widget`, `default_phase_h_state_is_off`. Multi-letter words like `_phase_prefixed_*` are deliberately not flagged.

The checks live in `xtask/src/conventions_identifier_rules.rs`. False positives (codec names like `h264_decoder`, hardware standards like `i2c_bus_init`) opt out via the `IDENTIFIER_ALLOWLIST` constant — every entry needs a justifying comment; the default move is to rename.

```rs
// WRONG                                    // CORRECT
fn h6_tab_overlay_dispatches() {}           fn tab_switcher_overlay_command_is_registered() {}
struct PhaseHState { … }                    struct PaneModesState { … }
const PHASE9_WIDGETS: …                     const WIDGET_COMMAND_IDS: …
fn apply_phase9_widget(…)                   fn apply_widget(…)
```

Test names matter most: phase-prefixed test names dominate CI output and become uninformative the moment the next phase lands. Describe what the test asserts (`tab_switcher_overlay_command_is_registered`) instead of the phase coordinate it lived under when written.

### `pub use` only in `lib.rs` (`pub-use-only-in-lib-rs`)

`pub use` is allowed only at the crate root (`lib.rs`) or in the binary's `main.rs`. Re-exports deeper in the tree split the crate's public surface across several files and force agents to chase chains — `crate::a::b::Foo` looks canonical until you discover it's actually re-exported from `crate::x::y::Foo`. Keeping all `pub use` in `lib.rs` makes the surface answerable by one read.

The check at `xtask/src/conventions_b_rules.rs::check_pub_use_only_in_lib_rs` rejects any line matching `pub use ` outside `lib.rs` / `main.rs`. The rare legitimate exception (an inner module that owns a re-export for path-stability reasons) opts out with an inline `// alias: <reason>` comment on the same line.

```rs
// WRONG: crates/foo/src/inner.rs
pub use crate::widget::Bar;                          // ❌ — move to lib.rs

// CORRECT: crates/foo/src/lib.rs
pub use widget::Bar;

// OPT-OUT (rare): inner.rs
pub use windows::Win32::UI::Input::Ime::HIMC; // alias: keep raw windows path out of consumer code
```

### No `use ... as Alias;` (`no-use-aliasing`)

`use foo::Bar as Baz;` makes the canonical name (`Bar`) unfindable from the alias (`Baz`) — agents grep for `Bar` to navigate, and `Baz` shadows the trail. The rule rejects aliased imports outside test code. When two imports genuinely collide (e.g. `Error` from two crates), opt out with an inline `// alias: <reason>` comment.

The check at `xtask/src/conventions_b_rules.rs::check_no_use_aliasing` matches `as` as a standalone token on any line beginning with `use ` / `pub use `, including brace-group aliases (`use foo::{Bar as Baz}`).

```rs
// WRONG
use std::path::PathBuf as Pb;                              // ❌ — qualify at the call site

// CORRECT (collision, opt-out)
use continuity_command::Error as CommandError; // alias: collides with crate::Error
```

## Errors

### `thiserror` per crate, `Error` in `error.rs`

```rs
// CORRECT: crates/buffer/src/error.rs
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("position {position:?} is outside the rope")]
    PositionOutOfBounds { position: Position },
    #[error(transparent)]
    Text(#[from] continuity_text::Error),
}
```

### `anyhow` only in `crates/app/src/main.rs` + `xtask/`

```rs
// CORRECT: crates/app/src/main.rs
fn main() -> anyhow::Result<()> { … }

// WRONG: any other crate
use anyhow::Result;             // ❌ define a thiserror enum instead
```

### No `unwrap()` / `panic!()` / `unreachable!()` in non-test code
`expect("invariant: …")` is allowed when the invariant is named and documented.

```rs
// CORRECT
let head = selection.first().expect("invariant: buffer always has at least one caret");

// WRONG
let head = selection.first().unwrap();  // ❌
```

## Identifiers

### Newtype every id
`BufferId`, `PaneId`, `WindowId`, `TabId`, `Revision`, `UndoGroupId` — all `u64` / `Uuid` underneath, type-incompatible at API surfaces. No raw `u64` parameters that mean "a buffer id".

```rs
// CORRECT
fn touch_buffer(&self, id: BufferId, ts_ms: i64);

// WRONG
fn touch_buffer(&self, id: u64, ts_ms: i64);   // ❌ accepts any u64
```

### No bare `TODO`
Issue-tag every TODO.

```rs
// CORRECT
// TODO(#247): clamp on overflow when wrap_width = 0.

// WRONG
// TODO: fix later.
```

## Concurrency

- Channels typed and directional. No `EventBus`, no string topics.
- `Mutex` only with a justifying doc comment naming the contention region (theme cache, font cache today).
- Unbounded channels forbidden outside startup; hot-path sends are `try_send` with explicit overflow policies.
- No `async fn`. `tokio` in `Cargo.lock` is a violation.
- Single-writer rule: every mutable state names its owning thread in a doc comment.

```rs
// CORRECT
/// Owned by the per-window UI thread.
pub(crate) caret_blink_active: bool,
```

## Imports

- No glob `use foo::*;` outside `#[cfg(test)]`.
- No cross-layer `pub use` — explicit imports keep the dependency graph legible.

## Performance gates

`cargo xtask ci` runs:
- `fmt --check`
- `clippy --workspace --all-targets -- -D warnings`
- `check --workspace --all-targets`
- `test --workspace`

`cargo xtask conventions` checks the static rules above. Both must be green pre-push.

`dhat` asserts zero new heap alloc per keystroke in steady state. `criterion` benches enforce keypress→pixel ≤8 ms p99 etc.

## Commits + hooks

### Conventional commits
`feat(scope): …` · `fix: …` · `docs: …` · `test: …` · `chore: …` · `refactor: …` · `perf: …` · `build: …` · `ci: …` · `style: …` · `revert: …`.

### Hooks
Activate via `cargo xtask install-hooks` → `git config core.hooksPath .githooks`.
- `pre-commit` → `xtask conventions && xtask fmt`
- `pre-push` → `xtask conventions && xtask ci`
- `commit-msg` → `xtask check-commit-msg "$1"`

Never bypass hooks (`--no-verify` is forbidden unless the user explicitly asks).

## Documentation

- `#![warn(missing_docs)]` at every crate root.
- Doc tests where the API is non-obvious.
- Crate-root `lib.rs` doc string describes the crate's responsibility in 2–3 sentences.

## Anti-patterns to resist

- Generic traits with one impl — delete them.
- `Backend` / `Renderer` / `Platform` abstraction layers — Windows-only target, single rope library, single graphics API.
- `EventBus` — use typed channels.
- `BufferBuilder` / `WindowBuilder` for ≤4-parameter constructors — `Self::empty()` + `Self::from_text(&str)` suffices.

## Tests

- Unit tests `#[cfg(test)] mod tests` next to code.
- Cross-crate integration in `crates/<x>/tests/`.
- `tempfile` for filesystem-touching tests.
- `Clock` trait for time; tests use `FakeClock`.
- Never network in tests.
- `crates/test_support/tests/canary.rs` must always pass — it's the smoke test for every regression that matters.
