# 00_OVERVIEW

## System purpose
- Native Win32 markdown notes editor in Rust — plain text + live preview, every keystroke durable, saving = export only.

## Surfaces
- Runtime: single process, one Win32 window per top-level surface, one UI thread per window, one shared core thread, decoration worker pool, persistence thread, file-I/O thread.
- Data: SQLite (WAL mode, bundled `≥3.51.3`), `%APPDATA%\continuity\continuity.db`. Hot-mirrored every 15 minutes to `%LOCALAPPDATA%\continuity\backups\`.
- Integrations: Windows DirectWrite + Direct2D + DXGI swap chain, IVirtualDesktopManager (COM), Windows ISpellChecker.
- Deployment: single stripped native Windows binary ≤9 MiB, zip-portable, no installer required.

## Doc map

### Structural
- [Architecture](architecture.md) — runtime model, layer graph, thread map.
- [Concurrency](concurrency.md) — single-writer rule, channel topology, revision discipline.
- [Data model](data_model.md) — SQLite schema, key types, ID newtypes.
- [Defaults](defaults.md) — product defaults and behavioral policy.
- [Interfaces](interfaces.md) — `Command` / `Context` / `EditorMessage` / `EditEvent` contracts.
- [Motion](motion.md) — functional animation, timing, and reduced-motion policy.
- [Performance](performance.md) — budgets (§spec 15) and the caches that protect them.
- [Principles](principles.md) — product ethos and non-negotiable interaction rules.
- [Public release](public-release.md) — public staging repo, release artifacts, GitHub Releases flow.
- [SDK contract](sdk-contract.md) — package names, ownership, targets, MSRV, compatibility, and version trains.
- [SDK release](sdk-release.md) — canonical SDK manifest, immutable bundle, trusted publishing, SBOM, and rollback.

### Features
- [Autocorrect](features/autocorrect.md) — user-editable rules and trigger detection.
- [Buffer](features/buffer.md) — rope, revisions, selections, undo tree.
- [Buffer-history tab](features/buffer-history-tab.md) — persisted-buffer swimlane timeline.
- [Caret presentation](features/caret.md) — shape, blink, jump glow, motion tween, sticky column.
- [Clipboard](features/clipboard.md) — copy, cut, literal/rich paste, paste history, and RTF copy.
- [Command system](features/command-system.md) — `CommandId`, `Context`, predicates, and dispatch.
- [Cross-platform desktop](features/cross-platform-desktop.md) — Electron ownership, host durability, distribution, and budgets.
- [Decoration](features/decoration.md) — tree-sitter incremental parse and Markdown spans.
- [Display map](features/display-map.md) — source ↔ display projection, folds, replacements, and soft wrap.
- [Embeddable Windows control](features/embeddable-windows-control.md) — child-control ownership and native host contract.
- [File I/O](features/file-io.md) — open, export, drag-drop, encoding, and external changes.
- [File tree](features/file-tree.md) — bounded folder browser and safe file-open routing.
- [Image paste](features/image-paste.md) — clipboard/drop image storage and inline rendering.
- [Keymap](features/keymap.md) — TOML bindings, chord sequencing, and conflict policy.
- [Minimap](features/minimap.md) — scaled-text thumbnail and viewport indicator.
- [Outline sidebar](features/outline-sidebar.md) — heading navigation and Markdown TOC.
- [Overlays](features/overlays.md) — palette, find, quick-open, and goto surfaces.
- [Panes, tabs, windows](features/panes-tabs-windows.md) — pane tree, MRU, and virtual desktops.
- [Persistence](features/persistence.md) — edit log, snapshots, recovery, trash, and hot backup.
- [Previous-buffer browser](features/previous-buffer-browser.md) — recent/closed-buffer navigation and recovery.
- [Rendering](features/rendering.md) — DirectWrite layout cache and Direct2D paint pipeline.
- [Search](features/search.md) — literal/regex find, replace, and fuzzy selection.
- [Selections + edits](features/selection-edits.md) — multi-cursor, block kind, `SelectionEdit` planner.
- [Settings](features/settings.md) — `settings.toml`, hot reload, validation.
- [Spell check](features/spell-check.md) — Windows spell service and per-buffer policy.
- [Tables](features/tables.md) — pipe-table projection, cell borders, and formulas.
- [Theme](features/theme.md) — TOML themes, required keys, and hot reload.
- [Tutorial](features/tutorial.md) — first-launch and command-driven tutorial surface.
- [Vaults](features/vaults.md) — marked folders, continuous export, and folder-scoped configuration.
- [Web Component](features/web-component.md) — semantic browser input, DOM projection, host events, and accessibility.
- [Touch input](features/touch-input.md) — coarse-pointer shield, projection-owned long-press selection, drag auto-scroll, selection action bar, and clipboard fallbacks.

### Technical (code organization)
- [Technical index](../technical/00_INDEX.md) — complete technical-doc inventory.
- [Crate inventory](../technical/crates.md) — crate responsibilities and key paths.
- [Import boundaries](../technical/import-boundaries.md) — allowed dependency direction.
- [Conventions](../technical/conventions.md) — code organization and validation rules.
- [Selection-edit dispatch flow](../technical/selection-edit-flow.md) — input through planner, undo, and persistence.
- [Paint frame flow](../technical/paint-flow.md) — native layout and render pipeline.
- [Testing and performance](../technical/testing-and-perf.md) — gate tiers, budgets, snapshots, and history.
- [Trace guide](../technical/trace-guide.md) — native diagnostics and regression workflow.
- [WASM SDK](../technical/wasm-sdk.md) — portable closure, npm facade, packaging, and browser budgets.
- [Native-language SDK](../technical/native-sdk.md) — Cargo, C ABI, Python, and clean-consumer gates.
- [Desktop Web implementation](../technical/desktop-web.md) — Electron source map, packaging, and artifact smoke.
- [Editor bake-off](../technical/editor-bakeoff.md) — reproducible local competitor comparison harness.

### Generated references
- [Generated docs map](../generated/README.md) — regenerated mechanical facts and drift commands.
- [Structured manifest](../generated/index.json) — tool-readable crates, modules, APIs, commands, settings, tests, schema, source paths.
- [Repo map](../generated/REPO_MAP.md) — compact code-localization overview.
- [Symbol maps](../generated/symbols/) — per-crate symbol → source/tests/config/commands/schema hints.

### Active plans (`.docs/development/`)
- [Embeddable + cross-platform roadmap](../development/embeddable_cross_platform_roadmap.md)
- [Code-file syntax highlighting](../development/code_file_syntax_highlighting_plan.md) — native desktop first; embedded, Web, and Electron deferred.
- [Release operations activation](../development/release_operations.md)
- [Embeddable Windows editor control](features/embeddable-windows-control.md)
- [Conventions reference](../development/development_conventions.md)
- [Code organization reference](../development/code_organization.md)
- [Deterministic documentation + hooks guide](../development/deterministic_documentation_and_hooks_guide.md)

### Historical plans and evidence (`.docs/development/archive/`)
- [Former long-form spec](../development/archive/spec.md)
- [Historical native roadmap](../development/archive/roadmap.md)
- [Historical performance frontier](../development/archive/roadmap_v5.md)
- [Historical development log](../development/archive/development_log.md)
- [Embeddable architecture baseline](../development/archive/embeddable_baseline_2026-07-16.md)
- [Embeddable native trace matrix](../development/archive/embeddable_trace_matrix_2026-07-16.md)
- [Embeddable browser presentation evidence](../development/archive/embeddable_presentation_spike_2026-07-17.md)
- [Future updates queue](../development/archive/roadmap_v2.md)
- [Unwired features](../development/archive/unwired_features.md)

## Global invariants
- **Single-writer per domain.** Each piece of mutable state names one owning thread; everything else sees `Arc`-snapshots stamped with `Revision`.
- **Source bytes are canonical.** Undo, persistence, search, file I/O speak source bytes. The display map is a derived projection — removing it yields a degraded but correct editor.
- **No async runtime.** No `tokio`, no `async-std`, no `async fn`. Sync code on threads + `crossbeam-channel` everywhere.
- **Channels typed and directional.** No event bus, no string topics; `Sender<EditorMessage>` / `Receiver<EditEvent>` are the only inter-thread paths.
- **Revision drops staleness.** A worker result carrying `Revision(n)` is discarded by the UI when the buffer has advanced past `n`. No callbacks, no version-check locks.
- **Newtype every id.** `BufferId`, `PaneId`, `WindowId`, `TabId`, `Revision`, `UndoGroupId` — `u64`/`Uuid` underneath but type-incompatible at API surfaces.
- **No file > 600 lines.** Unconditional — no per-file exemption mechanism. Split by responsibility into siblings (`foo.rs` + `foo/<helper>.rs`).
- **Every keystroke is safe.** Durable within 400 ms p99; recovery replay halts at first checksum mismatch with a user-visible banner, never silently.

## Key trade-offs
- **Native Windows remains concrete while the engine becomes portable.** ⇒ DirectWrite/Direct2D stays on the Windows adapter; shared edit/projection semantics move behind a synchronous storage-neutral facade rather than weakening the native path.
- **No plugin runtime.** ⇒ Commands and keymaps are data, behaviors are code, extension model is recompile-and-fork ⇒ native binary stays ≤9 MiB and there is no sandbox to worry about.
- **No FTS5 content index / quick-open** (Phase B / decisions §K). ⇒ live find uses direct literal/regex scans over current buffers; `Ctrl+O` opens a native file dialog instead ⇒ smaller surface, no index drift.
- **Display map projection** (Phase 17.5). ⇒ The layout never holds bytes that aren't supposed to be visible (markers, fence ticks, list-bullet glyphs are `Hidden`/`Replace`) ⇒ reveal is structural, not painted-over; layout widths are honest.
- **One mutable owner per `Engine`.** ⇒ Native Windows selects the core thread;
  embedded hosts select their own thread. Every other thread sees revisioned
  snapshots ⇒ zero lock contention on the hot path.
- **WASM stays synchronous; the Web Component is a host adapter.** ⇒ One
  JavaScript agent owns the engine and component. The semantic textarea and
  DOM projection provide browser input/presentation, while persistence and
  durability stay host-owned. The native Windows durability path is unchanged.
