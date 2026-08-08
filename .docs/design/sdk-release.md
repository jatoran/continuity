# SDK Release

## Scope

- In: canonical SDK version, immutable local bundle, registry workflows,
  attestations, SBOM, public staging, install/rollback policy.
- Out: desktop `v*` releases, registry account ownership, recovery secrets,
  Apple notarization, Linux signing identity.

## Vocabulary

- **SDK release**: Cargo engine closure, npm visual package, Python wheel, and
  C ABI archive sharing one version.
- **Stage**: build/test/package once; no registry mutation.
- **Publish**: approval-gated upload of the staged bundle represented by
  `release-manifest.json`.
- **Activation**: human configuration of registry owners, protected GitHub
  environments, and trusted publishers.

## Invariants

- `sdk/release.toml` is the canonical SDK version, channel, MSRV, repository,
  package set, npm tags, Python floor, and C ABI target.
- Cargo/npm/Python/C wrapper versions and public-repository metadata must match
  the canonical manifest. `cargo xtask sdk-release-check` rejects drift.
- SDK tags use `sdk-v<version>`; native desktop tags remain `v<version>`.
- `workflow_dispatch` stages only. Registry publication requires a protected
  `sdk-v*` tag plus approval in each registry environment.
- Publish jobs download the stage job's artifact. npm, PyPI, the C archive,
  checksums, SBOM, and GitHub assets are not rebuilt.
- Cargo necessarily packages during `cargo publish`. The workflow packages
  once immediately before upload and requires its SHA-256 to equal the staged
  `.crate`; it repeats the comparison after Cargo creates the uploaded file.
- No registry token, certificate, recovery code, or account secret belongs in
  the repository. OIDC trusted publishing is the steady-state credential path.

## Local commands

```powershell
cargo xtask sdk-release-check
cargo xtask sdk-release-dry-run --allow-dirty
cargo xtask sdk-release-verify
```

Omit `--allow-dirty` for a release. The dry run invokes packed Cargo, C, Python,
and npm/WASM gates; builds a Windows C archive; emits a CycloneDX 1.5 inventory;
and writes the immutable bundle under `target/sdk-release/sdk-v<version>/`.
`latest-output.txt` points at the newest local bundle.

Bundle contents:

```text
continuity-text-<version>.crate
continuity-buffer-<version>.crate
continuity-engine-<version>.crate
continuity-editor-<version>.tgz
continuity_editor-<version>-cp310-abi3-win_amd64.whl
continuity-engine-c-<version>-windows-x86_64.zip
continuity-sdk-<version>.cdx.json
SHA256SUMS.txt
release-manifest.json
```

## CI and publication

`.github/workflows/sdk-release.yml` has one build job and four independent,
protected publish destinations:

| Environment | Mutation |
|---|---|
| `npm` | exact `.tgz` → `@continuity-editor/editor`, preview tag `next` |
| `crates-io` | checked `.crate` closure in text → buffer → engine order |
| `pypi` | exact Windows `abi3` wheel → `continuity-editor` |
| `sdk-release` | exact bundle → GitHub SDK release |

The tagged stage obtains GitHub OIDC attestations for build provenance and the
CycloneDX SBOM. npm and PyPI use trusted publishing. crates.io uses
`rust-lang/crates-io-auth-action`; crates.io requires a manual first release
before its trusted publisher can be attached.

The workflow is copied to the public staging repository by
`scripts/sync-public.ps1`. Trusted publishers must name that public repository,
the exact `sdk-release.yml` filename, and their matching environment.

## Installation contract

The public support matrix is intentionally asymmetric:

```text
cargo add continuity-engine                  # headless Rust engine
npm install @continuity-editor/editor@next   # visual Web Component + WASM
python -m pip install continuity-editor      # headless Windows Python binding
uv pip install continuity-editor             # same PyPI wheel
```

React hosts use the optional subpath exported by the same npm artifact:
`import { ContinuityEditor } from "@continuity-editor/editor/react"`. React is
not installed for framework-neutral or headless consumers.

The C ABI is a checked Windows x86-64 GitHub Release archive. The native Win32
desktop remains the durable SQLite product and is not installed by an SDK
package. `continuity_ui::EditorControl` remains a source/workspace control.

## Rollback

- npm: deprecate the bad version and move `next`/`latest` to a known-good one.
- Cargo: yank the bad version; publish a corrected version. Never overwrite.
- PyPI: publish a corrected version. Do not attempt to replace an uploaded file.
- GitHub: mark the SDK release withdrawn and publish a corrected `sdk-v*` tag.
- Compromise: revoke the trusted publisher or credential first, then follow the
  registry-specific rollback. A yank/deprecation is not secret revocation.

## Activation gate

Repository automation does not prove registry ownership. Before the first tag,
complete `.docs/development/release_operations.md`, configure required reviewers
on every GitHub environment, perform the crates.io bootstrap publish, and run
clean network-backed consumers against every advertised coordinate.

## Key files

| File | Responsibility |
|---|---|
| `sdk/release.toml` | Canonical SDK release identity |
| `xtask/src/sdk_release.rs` | Build-once staging orchestration |
| `xtask/src/sdk_release_manifest.rs` | Wrapper/version/repository verification |
| `xtask/src/sdk_release_artifact.rs` | SHA-256 manifest and tamper checks |
| `xtask/src/sdk_release_sbom.rs` | CycloneDX release-closure inventory |
| `.github/workflows/sdk-release.yml` | OIDC stage/publish release train |
| `scripts/sync-public.ps1` | Public source and workflow staging |

## External references

- [GitHub artifact attestations](https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/use-artifact-attestations)
- [npm trusted publishing](https://docs.npmjs.com/trusted-publishers/)
- [Cargo publishing](https://doc.rust-lang.org/cargo/reference/publishing.html)
- [PyPI trusted publishing](https://docs.pypi.org/trusted-publishers/)
