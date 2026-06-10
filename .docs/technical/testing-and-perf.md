# Testing, perf gates, and perf history (Phase 17.9 reference)

This is the authoritative reference for the assembled testing + perf
story shipped in Phase 17.9. When this doc disagrees with
`xtask/src/conventions.rs`, `xtask/src/bench.rs`, `xtask/src/perf_history.rs`,
or `.githooks/`, **the code wins** — open a PR to bring this doc back
in line.

Generated mechanical inventories live in
[`../generated/GATES.md`](../generated/GATES.md) and
[`../generated/TEST_INDEX.md`](../generated/TEST_INDEX.md). This doc
owns the tier contracts, budgets, and rationale; generated docs own the
current command/test file lists.

## Tier model

Three tiers, each with a wall-time target and a contract for what it
catches.

| Tier | Trigger | Command | Target wall-time | Catches |
|------|---------|---------|------------------|---------|
| **fast** | every commit | `.githooks/pre-commit`: `xtask conventions` + `xtask ci` | < 90 s | conventions violations, fmt drift, clippy lints, type errors, unit-test regressions |
| **fat** | every push | `.githooks/pre-push`: `+ xtask bench-fast` + `xtask e2e-smoke` + `xtask snapshot-canary` | 5–12 min | the above plus perf-gate regressions, the cheapest E2E pair, pixel-canary regressions |
| **release** | manual / CI | `xtask check-all` (`= test-all + bench + perf-snapshot`) | well under 30 min | the above plus the full E2E set, the full perf-gate set, a fresh perf snapshot |

`xtask agent-check` runs `docs-check` before `check-all`, then emits a
structured-JSON outcome on stdout for agent consumption; the
human-readable summary goes to stderr; exit code mirrors the JSON
`pass` field.

## xtask command catalogue

Run `cargo xtask help` for the canonical list. For hook/CI membership
and perf-gate command hints, use
[`../generated/GATES.md`](../generated/GATES.md). Phase 17.9 adds:

| Command | What it does |
|---------|--------------|
| `docs` | Regenerates `.docs/generated/` from source-controlled mechanical facts. |
| `docs-check` | Regenerates generated docs in memory; fails on drift, missing headers, stale generated files, or generated files over 600 lines. |
| `snapshot-canary` | Runs the §D pixel canary in compare mode. |
| `snapshot-update` | Re-runs the canary with `CONTINUITY_PIXEL_CANARY_UPDATE=1`; rewrites every `crates/render/tests/fixtures/*.hash`. Reviewer eye-balls the visual diff and commits. |
| `e2e-smoke` | Runs `e2e_smoke` + `e2e_pane_split` (the two cheapest E2E targets). |
| `e2e-stress` | Runs `e2e_stress` (`--nocapture`): a long-usage crash hunt — 3 hidden windows sharing one core, split panes, and a seeded random storm of typing / delete / paste / click / scroll / pane-churn / DPI-flip ops. Tune length with `CONTINUITY_STRESS_OPS` (default 800) and reproduce a failure from the printed `CONTINUITY_STRESS_SEED`. Survival-only (every op returns, every buffer snapshot stays walkable, every window still paints); not in any hook tier. |
| `test-all` | `ci` + every named E2E + `snapshot-canary`. |
| `check-all` | `test-all` + full `bench` + `perf-snapshot`. The "is this commit shippable?" one-shot. |
| `agent-check` | `check-all` with a `CheckAllOutcome` JSON record on stdout. |
| `perf-snapshot` | Runs every gate, aggregates per-gate JSON into `target/perf/snapshot-<sha>.json`. |
| `perf-history-append` | Appends the latest snapshot to `.perf/history.jsonl` (idempotent on `(sha, host_id)`). |
| `perf-report [--last N]` | Prints a per-gate p99 / p99.9 / jitter trend table. |
| `perf-compare --baseline <sha>` | Compares the latest snapshot against `<sha>` from history; non-zero exit on >10% p99 or >20% p99.9 growth. |

## Perf gates

Every gate enforces `p99 ≤ budget` AND `p99.9 ≤ 2 × budget` (the
variance-tail check landed in §B4 and lives in
[`continuity_test_support::percentiles::assert_within_budget`](../../crates/test_support/src/percentiles.rs)).

| Gate | Crate | Test target | Spec budget (p99) | CI ceiling | Tier |
|------|-------|-------------|-------------------|------------|------|
| buffer apply | `continuity-buffer` | `perf_gates` | 2 ms | 2 ms | fast |
| core apply_edit | `continuity-core` | `perf_gates` | 4 ms | 4 ms | fast |
| decoration incremental parse | `continuity-decorate` | `perf_gates` | 1 ms | 4 ms | fast |
| display-map build | `continuity-display-map` | `perf_gates` | 2 ms | 8 ms | fast |
| persist edit-to-durable | `continuity-persist` | `perf_gates` | 400 ms | 400 ms | fast |
| D2D draw-submission | `continuity-render` | `perf_gates` | 2 ms | 8 ms | fast |
| WM_PAINT → frame-ready | `continuity-ui` | `perf_gates` | 2 ms | 8 ms | fat |
| search find-in-N | `continuity-search` | `perf_gates` | 80 ms | 80 ms | fat |
| memory empty session | `continuity-test-support` | `perf_gates_memory_empty` | 40 MB | 40 MB | fast |
| memory 50 buffers | `continuity-test-support` | `perf_gates_memory_50` | 90 MB | 90 MB | fast |
| memory 200 buffers / 50 MB | `continuity-test-support` | `perf_gates_memory_200` | 180 MB | 180 MB | fat |

The keypress→pixel 8 ms total breaks down per spec §15:

```
core apply_edit (≤ 4 ms) + decoration (≤ 1 ms)
  + draw-submission (≤ 2 ms) + present (≤ 1 ms) = 8 ms p99
```

## Pixel canary

`crates/render/tests/pixel_canary.rs` renders 20 fixtures via WARP
(software rasterizer, `Renderer::for_hwnd_warp`), captures the swap
chain back buffer (`Renderer::capture_back_buffer`), hashes with
blake3, and compares against `crates/render/tests/fixtures/<name>.hash`.
Determinism mitigations: WARP for byte-stable rasterization,
`D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE` (forced inside `for_hwnd_warp`)
to neutralize ClearType subpixel jitter, and the Cascadia Mono font
family (ships with Windows 10+).

When a deliberate render change lands and breaks hashes, run
`cargo xtask snapshot-update` and commit the regenerated `.hash`
files — eye-ball the resulting visuals before committing.

## Perf history format

`.perf/history.jsonl` is a tracked JSONL file. One snapshot per line:

```json
{"git_sha":"…","timestamp_unix":1778650645,"host_id":"…","rustc_version":"…","samples":{"buffer apply":{"label":"buffer apply","p50_us":0,…,"jitter":1.166667,"sample_count":1024},…}}
```

Append cadence: CI on every push to `main` (`perf-history` job).
Local development can append by running `cargo xtask perf-snapshot`
followed by `cargo xtask perf-history-append`. Idempotent on
`(git_sha, host_id)` — re-running on the same commit replaces the
matching row.

`cargo xtask perf-report` prints a per-gate trend table. `host_id`
defaults to `$COMPUTERNAME` / `$HOSTNAME`; CI overrides it via
`CONTINUITY_HOST_ID=github-actions-windows-latest` so cross-host
samples don't false-trigger regressions.

## E2E harness

`crates/test_support/src/win32_harness.rs::Win32Harness` spawns a real
hidden `continuity_ui::Window` on a worker thread driving the
production wndproc + Renderer. The harness keymap binds modifier-free
chords (F1–F5, plus `backspace`/`delete`) so tests drive commands
through the production dispatch path without modifier-state races. The
shipped tests live in `crates/ui/tests/`:

- `e2e_smoke` — typing produces text in the buffer.
- `e2e_pane_split` — F1 chord splits panes; buffer count doubles.
- `e2e_multi_window` — a second window via `spawn_sharing` reflects
  edits made through the first.
- `e2e_settings_live_reload` — `WindowControl::ConfigChanged` fired
  through the harness's control channel applies cleanly.
- `e2e_stress` — long-usage crash hunt (`cargo xtask e2e-stress`): 3
  windows + split panes + a seeded random op storm; asserts survival
  (no panic, buffers stay walkable, windows still paint). Not in any
  hook tier; run on demand when chasing intermittent crashes.

The pixel canary deliberately does **not** use the C1 harness — it
needs WARP for determinism, the harness drives hardware D3D for
production fidelity. Different tools for different jobs.

## Crash-recovery E2E

`crates/app/tests/e2e_crash_recovery.rs` spawns the production
`continuity.exe` against a tempdir-backed DB via two new env
contracts:

- `CONTINUITY_DATA_DIR=<dir>` — persist::paths overrides for both the
  live DB and backups (lives next to the eventual Phase-18
  `--portable` flag).
- `CONTINUITY_E2E_INSERT=<text>` — app/main.rs hook: open one buffer,
  apply the insert, sleep 600 ms (1.5× the spec §15 durability
  ceiling), drop a marker file `<datadir>/.e2e_inserted` containing
  the buffer UUID, then sleep forever waiting for `Child::kill` (=
  `TerminateProcess` on Windows).

The test polls for the marker, kills the child, reopens the DB
in-process, replays snapshot + edit log with checksum verification,
asserts the recovered rope equals the typed text.

`crates/persist/tests/e2e_kill_during_write.rs` is the narrower
in-process companion: drives the persist thread directly, drops it
mid-batch, reopens, asserts every Ok-returned `append_edit` survived.

## CI pipeline

`.github/workflows/ci.yml`:

- **check-all** (`windows-latest`, every push + PR): runs
  `xtask conventions` + `xtask check-all`. Uploads
  `target/perf/snapshot-*.json` as an artifact.
- **perf-history** (push to `main` only): downloads the artifact, runs
  `perf-history-append`, commits `.perf/history.jsonl` with `[skip ci]`.
- **pr-perf-compare** (PRs only): downloads the artifact, runs
  `perf-compare --baseline $(git merge-base origin/main HEAD)`, posts
  the result as a sticky PR comment. `continue-on-error: true` for
  the initial soak window — flip to blocking after a week of clean data.
