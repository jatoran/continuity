# 00_INDEX — Technical docs

Code-organization reference. Tells agents where the code lives, what crate owns what, and what patterns new code must follow. Complements the design docs in `.docs/design/` (which describe *what the system does and why*).

Use `.docs/generated/` for current mechanical facts before broad source search. Use handwritten technical docs for rules, boundaries, and modification patterns.

## Files

- [crates.md](crates.md) — per-crate inventory: responsibility, key files, dependencies.
- [import-boundaries.md](import-boundaries.md) — what may import what; layering rules.
- [conventions.md](conventions.md) — file-length cap, no-mod-rs, no-unwrap, naming, commits.
- [selection-edit-flow.md](selection-edit-flow.md) — keystroke → planner → undo group → persist (code walkthrough).
- [paint-flow.md](paint-flow.md) — WM_PAINT → frame display → layout cache → D2D draw (code walkthrough).

## Generated references

- [../generated/README.md](../generated/README.md) — generated-doc map, drift commands, stale-route checks.
- [../generated/index.json](../generated/index.json) — machine-readable generated manifest for tool retrieval.
- [../generated/REPO_MAP.md](../generated/REPO_MAP.md) — compact crate map, command/settings/schema summary, localization routes.
- [../generated/CRATES.md](../generated/CRATES.md) — workspace members, direct deps, module/re-export counts.
- [../generated/FILE_TREE.md](../generated/FILE_TREE.md) — compact source/doc/test tree.
- [../generated/FILE_HEALTH.md](../generated/FILE_HEALTH.md) — line-cap and missing README signals.
- [../generated/COMMANDS.md](../generated/COMMANDS.md) — registered commands, default keys, palette flags.
- [../generated/SETTINGS.md](../generated/SETTINGS.md) — settings keys, Rust fields, types, defaults.
- [../generated/GATES.md](../generated/GATES.md) — xtask commands, hook/CI membership, perf-gate commands.
- [../generated/TEST_INDEX.md](../generated/TEST_INDEX.md) — test files by crate plus integration, bench, e2e, canary, and golden command hints.
- [../generated/THEME_KEYS.md](../generated/THEME_KEYS.md) — required theme keys, bundled theme coverage, typed accessors.
- [../generated/PERSIST_SCHEMA.md](../generated/PERSIST_SCHEMA.md) — SQLite schema version, migration tables, indexes, alters.
- [../generated/MESSAGES.md](../generated/MESSAGES.md) — typed message/event/control enum variants and payload summaries.
- [../generated/SELECTION_EDITS.md](../generated/SELECTION_EDITS.md) — `SelectionEdit` variants, helper enums, planner routing.
- [../generated/modules/](../generated/modules/) — per-crate module/file inventory.
- [../generated/api/](../generated/api/) — per-crate top-level public API inventory.
- [../generated/symbols/](../generated/symbols/) — per-crate symbol locations plus related tests/settings/commands/schema hints.

## Where else to look
- `.docs/design/` — system documentation (intent + invariants per feature).
- `.docs/development/spec.md` — source of truth (long-form).
- `.docs/development/code_organization.md` — overlapping long-form layer graph + abstraction rules.
- `.docs/development/development_conventions.md` — overlapping long-form conventions.
- `crates/<x>/README.md` — one-paragraph crate purpose.
- `xtask/src/conventions.rs` — machine-checkable rules.

The `.docs/technical/` files here are **agent-optimized** (short, code-pointered, code-example-heavy). When they conflict with the long-form references in `.docs/development/`, the long-form wins as the canonical source.
