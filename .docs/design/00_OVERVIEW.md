# 00_OVERVIEW

## System purpose
- Native Win32 markdown notes editor in Rust — plain text + live preview, every keystroke durable, saving = export only.

## Surfaces
- Runtime: single process, one Win32 window per top-level surface, one UI thread per window, one shared core thread, decoration worker pool, persistence thread, file-I/O thread.
- Data: SQLite (WAL mode, bundled `≥3.51.3`), `%APPDATA%\continuity\continuity.db`. Hot-mirrored every 15 minutes to `%LOCALAPPDATA%\continuity\backups\`.
- Integrations: Windows DirectWrite + Direct2D + DXGI swap chain, IVirtualDesktopManager (COM), Windows ISpellChecker.
- Deployment: single stripped binary ≤8 MB, zip-portable, no installer required.

## Doc map

### Structural
- [Architecture](architecture.md) — runtime model, layer graph, thread map.
- [Concurrency](concurrency.md) — single-writer rule, channel topology, revision discipline.
- [Data model](data_model.md) — SQLite schema, key types, ID newtypes.
- [Interfaces](interfaces.md) — `Command` / `Context` / `EditorMessage` / `EditEvent` contracts.
- [Performance](performance.md) — budgets (§spec 15) and the caches that protect them.
- [Public release](public-release.md) — public staging repo, release artifacts, GitHub Releases flow.

### Features
- [Buffer](features/buffer.md) — rope, revisions, selections, undo tree.
- [Selections + edits](features/selection-edits.md) — multi-cursor, block kind, `SelectionEdit` planner.
- [Persistence](features/persistence.md) — edit log, snapshots, recovery, trash, hot backup.
- [Decoration](features/decoration.md) — tree-sitter incremental parse, markdown spans, headings, autolink.
- [Display map](features/display-map.md) — source ↔ display projection (hide / replace / fold / soft-wrap).
- [Rendering](features/rendering.md) — DirectWrite layout cache, Direct2D paint pipeline.
- [Command system](features/command-system.md) — `CommandId`, `Context`, `ContextPredicate`, dispatch.
- [Keymap](features/keymap.md) — TOML keymap, chord sequencing, conflict checker.
- [Theme](features/theme.md) — TOML themes, required key set, hot reload.
- [Panes, tabs, windows](features/panes-tabs-windows.md) — pane tree, MRU vs positional, virtual desktops.
- [Search](features/search.md) — find bar, replace, regex helper, literal/regex dispatcher, fuzzy picker.
- [Overlays](features/overlays.md) — palette, find bar, quick-open, goto-line, goto-heading.
- [File I/O](features/file-io.md) — open / save, drag-drop, encoding, external-change banner.
- [File tree](features/file-tree.md) — left folder browser, bounded directory listing, safe file-open routing.
- [Clipboard](features/clipboard.md) — copy / cut / paste, smart paste, paste history, RTF copy.
- [Spell check](features/spell-check.md) — Windows ISpellChecker integration, per-buffer toggle.
- [Settings](features/settings.md) — `settings.toml`, hot reload, validation.
- [Autocorrect](features/autocorrect.md) — user-editable rule store, trigger detection.
- [Caret presentation](features/caret.md) — shape, blink, jump glow, motion tween, sticky column.
- [Minimap](features/minimap.md) — scaled-text right-edge thumbnail + viewport indicator.
- [Buffer-history tab](features/buffer-history-tab.md) — swimlane timeline of every persisted buffer (one row per buffer, snapshot dots on a shared time axis); complement to the previous-buffer browser overlay.

### Technical (code organization)
- [Crate inventory](../technical/crates.md)
- [Import boundaries](../technical/import-boundaries.md)
- [Selection-edit dispatch flow](../technical/selection-edit-flow.md)
- [Paint frame flow](../technical/paint-flow.md)
- [Conventions](../technical/conventions.md)

### Generated references
- [Generated docs map](../generated/README.md) — regenerated mechanical facts and drift commands.
- [Structured manifest](../generated/index.json) — tool-readable crates, modules, APIs, commands, settings, tests, schema, source paths.
- [Repo map](../generated/REPO_MAP.md) — compact code-localization overview.
- [Symbol maps](../generated/symbols/) — per-crate symbol → source/tests/config/commands/schema hints.

### Active plans (`.docs/development/`)
- [Source-of-truth spec](../development/spec.md)
- [Roadmap + phase status](../development/roadmap.md)
- [Development log](../development/development_log.md)
- [Conventions reference](../development/development_conventions.md)
- [Code organization reference](../development/code_organization.md)
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
- **Windows-only target.** ⇒ DirectWrite/Direct2D directly, no abstraction layer ⇒ best Windows text quality, no future cross-platform port.
- **No plugin runtime.** ⇒ Commands and keymaps are data, behaviors are code, extension model is recompile-and-fork ⇒ binary stays ≤8 MB and there is no sandbox to worry about.
- **No FTS5 content index / quick-open** (Phase B / decisions §K). ⇒ live find uses direct literal/regex scans over current buffers; `Ctrl+O` opens a native file dialog instead ⇒ smaller surface, no index drift.
- **Display map projection** (Phase 17.5). ⇒ The layout never holds bytes that aren't supposed to be visible (markers, fence ticks, list-bullet glyphs are `Hidden`/`Replace`) ⇒ reveal is structural, not painted-over; layout widths are honest.
- **`Buffer` mutation only on the core thread.** ⇒ One `EditorState` owner, every other thread sees `RopeSnapshot` clones ⇒ zero lock contention on the hot path, all coordination is by revision.
