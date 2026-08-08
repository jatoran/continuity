# Native-language SDK distributions

Milestone 9 packages the synchronous storage-neutral engine for Rust, C, and
Python without changing the native Windows desktop host or SQLite durability.

## Supported preview matrix

| Surface | Artifact | Supported target |
|---|---|---|
| Rust | `continuity-text`, `continuity-buffer`, `continuity-engine` `.crate` archives | Tier-1 Windows; engine remains portable |
| C | `continuity_engine.dll` + `continuity_engine.h` | Windows x86-64 MSVC, ABI major 1 |
| Python | `continuity_editor-0.2.5-cp310-abi3-win_amd64.whl` | CPython 3.10+ on Windows x86-64 |

Local commands never publish to a registry. The protected `sdk-v*` workflow
publishes only after registry ownership/trusted-publisher activation described
in `.docs/design/sdk-release.md`.

## One gate

```powershell
cargo xtask sdk-check
```

The gate performs:

1. Checks every exported `continuity_engine_*` symbol against the public header
   and stages the generated header as evidence.
2. Runs `cargo package --locked` and `cargo publish --dry-run` for the minimum
   publish closure. Until the lower crates exist in crates.io, buffer/engine
   packaging resolves checked local patches and uses `--no-verify`; the next
   clean-archive consumer is the build verification.
3. Audits archive paths against private docs, credentials, environment files,
   performance snapshots, and traces.
4. Extracts the produced `.crate` files, generates an isolated Cargo project,
   and builds/runs only against those extracted archives.
5. Builds `continuity_engine.dll` with `--profile release-sdk`, compiles the
   checked C consumer with MSVC `/W4 /WX`, and runs ABI/parity/callback/free and
   teardown checks. The gate resolves the newest installed x64 C++ toolchain
   through Visual Studio Installer's `vswhere`, so both Visual Studio 2022 and
   Visual Studio 2026 layouts are supported without hardcoded edition paths.
   Cargo is passed the same explicit x64 target whose artifact directory the
   consumer links, so a clean host does not depend on `CARGO_BUILD_TARGET`.
6. Builds the Python wheel through maturin, audits the zip entries, creates a
   clean venv, installs that wheel, and runs the shared parity fixture plus
   callback/snapshot/teardown checks.

Run directories are unique under `target/sdk-check/`; this avoids Windows file
sharing failures from antivirus, Python, or compiler handles.

`cargo xtask sdk-release-dry-run --allow-dirty` composes this evidence with the
npm tarball into one hashed, SBOM-bearing bundle. Release rehearsals omit
`--allow-dirty`; `cargo xtask sdk-release-verify` rehashes every staged file.

## Consume the newest local artifacts

For a local Rust host during development, depend directly on the storage-
neutral facade:

```toml
[dependencies]
continuity-engine = { path = "D:/PROJECTS/continuity/crates/engine" }
```

To test what will actually ship, run `cargo xtask sdk-check` and use the
extracted archives in the newest `target/sdk-check/<run>/cargo-archives/`
consumer, as the gate does. For Python, install the exact newest wheel into an
application environment:

```powershell
$run = Get-ChildItem target/sdk-check -Directory | Sort-Object LastWriteTime | Select-Object -Last 1
python -m pip install --force-reinstall (Get-ChildItem "$($run.FullName)/wheels/*.whl")
# Or, in a uv-managed environment:
uv pip install --reinstall (Get-ChildItem "$($run.FullName)/wheels/*.whl")
```

The wheel is headless and creates no Continuity database. The application owns
its save path and persistence policy. Use the npm Web Component in a Python
web-view when the application needs Continuity's visual surface.

## Ownership contracts

- Rust: caller-selected thread owns `Engine`; hosts own persistence and UI.
- C: creating thread owns the opaque handle. Matching free functions release
  Rust allocations. Same-handle calls during callbacks return
  `CONTINUITY_ENGINE_REENTRANT_CALL`. Destroy exactly once on the owner thread.
- Python: constructing thread owns `Editor`. `close()` is explicit;
  context-manager exit calls it. Calls after close raise `RuntimeError`.
- C/Python callbacks run after engine mutation and receive the accepted
  revision. Neither binding invokes callbacks while mutable engine state is
  borrowed.

## Key files

| File | Responsibility |
|---|---|
| `xtask/src/sdk.rs` | archive, header, external-consumer, wheel, and audit gate |
| `crates/c_api/include/continuity_engine.h` | checked public ABI header |
| `crates/c_api/src/api.rs` | panic-contained exported functions |
| `crates/c_api/tests/packed_consumer.c` | external C compatibility fixture |
| `bindings/python/pyproject.toml` | maturin/wheel metadata |
| `bindings/python/src/lib.rs` | Python `Editor`/`Snapshot` facade |
| `bindings/python/tests/packed_consumer.py` | clean-wheel fixture |
| `sdk/consumers/rust/main.rs` | extracted-archive Cargo fixture |

## Correct integration

```rust
// Correct: host owns one synchronous engine on its chosen writer thread.
let mut engine = continuity_engine::Engine::new();
let document = engine.open_buffer("host-owned text");
```

```rust
// Wrong: treating an in-memory mutation as durable host persistence.
let _batch = engine.apply_selection_edit(document, &edit, timestamp)?;
// A storage adapter must still commit the returned batch.
```
