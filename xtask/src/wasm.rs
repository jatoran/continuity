//! WebAssembly engine validation and npm artifact assembly.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};

mod consumer_tests;
mod generated_dir;
mod package_artifact;

use generated_dir::recreate_generated_dir;

const WASM_TARGET: &str = "wasm32-unknown-unknown";
const WASM_PACKAGE: &str = "continuity-wasm";
const WASM_PROFILE: &str = "release-wasm";

/// Compile the portable closure, run parity, and exercise the packed npm artifact.
pub(crate) fn check() -> Result<()> {
    let workspace = crate::release::workspace_root();
    let environment = WasmBuildEnvironment::discover(&workspace)?;
    ensure_tool_versions(&workspace, &environment)?;
    let tarball = validated_tarball(&workspace, &environment)?;
    eprintln!("wasm-check: PASS ({})", tarball.display());
    Ok(())
}

/// Exercise the packed Web Component in Chromium and the Electron IPC host.
pub(crate) fn browser_check() -> Result<()> {
    let workspace = crate::release::workspace_root();
    let environment = WasmBuildEnvironment::discover(&workspace)?;
    ensure_tool_versions(&workspace, &environment)?;
    let tarball = validated_tarball(&workspace, &environment)?;
    run_browser_consumer(&workspace, &tarball)?;
    run_electron_consumer(&workspace, &tarball)?;
    eprintln!("browser-check: PASS ({})", tarball.display());
    Ok(())
}

fn validated_tarball(workspace: &Path, environment: &WasmBuildEnvironment) -> Result<PathBuf> {
    run_portable_cargo(
        workspace,
        environment,
        &[
            "check",
            "--target",
            WASM_TARGET,
            "-p",
            "continuity-engine",
            "-p",
            "continuity-decorate",
            "-p",
            "continuity-display-map",
            "-p",
            WASM_PACKAGE,
        ],
    )?;
    run_cargo(
        workspace,
        &["test", "-p", WASM_PACKAGE, "--test", "parity_native"],
    )?;
    let tarball = package_with_environment(workspace, environment)?;
    run_clean_consumer(workspace, &tarball)?;
    Ok(tarball)
}

/// Build the release WASM module and assemble an installable npm tarball.
pub(crate) fn package() -> Result<()> {
    let tarball = package_tarball()?;
    eprintln!("npm tarball written to {}", tarball.display());
    Ok(())
}

/// Build the release WASM module and return the packed npm artifact path.
pub(crate) fn package_tarball() -> Result<PathBuf> {
    let workspace = crate::release::workspace_root();
    let environment = WasmBuildEnvironment::discover(&workspace)?;
    ensure_tool_versions(&workspace, &environment)?;
    package_with_environment(&workspace, &environment)
}

struct WasmBuildEnvironment {
    compiler: OsString,
    compiler_flags: OsString,
    wasm_bindgen: OsString,
}

impl WasmBuildEnvironment {
    fn discover(workspace: &Path) -> Result<Self> {
        let compiler = discover_clang()?;
        require_success(Command::new(&compiler).arg("--version"), "Clang probe")?;

        let include = workspace.join("crates/wasm/include");
        let required_flags = format!("-DNDEBUG -I \"{}\"", include.display());
        let compiler_flags = match env::var_os("CFLAGS_wasm32_unknown_unknown") {
            Some(existing) if !existing.is_empty() => {
                let mut combined = existing;
                combined.push(" ");
                combined.push(required_flags);
                combined
            }
            _ => OsString::from(required_flags),
        };
        Ok(Self {
            compiler,
            compiler_flags,
            wasm_bindgen: executable_name("wasm-bindgen"),
        })
    }

    fn apply(&self, command: &mut Command) {
        command.env("CC_wasm32_unknown_unknown", &self.compiler);
        command.env("CFLAGS_wasm32_unknown_unknown", &self.compiler_flags);
        command.env("CC_SHELL_ESCAPED_FLAGS", "1");
    }
}

fn discover_clang() -> Result<OsString> {
    if let Some(configured) = env::var_os("CC_wasm32_unknown_unknown") {
        return Ok(configured);
    }
    let normal = executable_name("clang");
    if Command::new(&normal).arg("--version").output().is_ok() {
        return Ok(normal);
    }
    #[cfg(windows)]
    {
        let msys = PathBuf::from(r"C:\msys64\clang64\bin\clang.exe");
        if msys.is_file() {
            return Ok(msys.into_os_string());
        }
    }
    bail!(
        "Clang is required to compile tree-sitter for {WASM_TARGET}; install it or set CC_wasm32_unknown_unknown"
    )
}

fn ensure_tool_versions(workspace: &Path, environment: &WasmBuildEnvironment) -> Result<()> {
    require_success(
        Command::new(executable_name("node")).arg("--version"),
        "Node.js probe",
    )?;
    let mut npm_probe = Command::new(executable_name("npm"));
    npm_probe.arg("--version");
    configure_npm(workspace, &mut npm_probe);
    require_success(&mut npm_probe, "npm probe")?;
    let expected = resolved_wasm_bindgen_version(workspace)?;
    let output = Command::new(&environment.wasm_bindgen)
        .arg("--version")
        .output()
        .context("running wasm-bindgen --version; install wasm-bindgen-cli")?;
    if !output.status.success() {
        bail!("wasm-bindgen --version failed");
    }
    let actual = String::from_utf8_lossy(&output.stdout);
    if !actual.split_whitespace().any(|part| part == expected) {
        bail!(
            "wasm-bindgen-cli {} does not match the resolved wasm-bindgen crate {}; install with `cargo install wasm-bindgen-cli --version {expected} --locked`",
            actual.trim(),
            expected
        );
    }
    Ok(())
}

fn resolved_wasm_bindgen_version(workspace: &Path) -> Result<String> {
    let lock = fs::read_to_string(workspace.join("Cargo.lock"))?;
    let document: toml::Value = toml::from_str(&lock)?;
    document["package"]
        .as_array()
        .and_then(|packages| {
            packages.iter().find_map(|package| {
                (package["name"].as_str() == Some("wasm-bindgen"))
                    .then(|| package["version"].as_str().map(ToOwned::to_owned))
                    .flatten()
            })
        })
        .ok_or_else(|| anyhow!("resolved wasm-bindgen package is missing from Cargo.lock"))
}

fn package_with_environment(
    workspace: &Path,
    environment: &WasmBuildEnvironment,
) -> Result<PathBuf> {
    run_portable_cargo(
        workspace,
        environment,
        &[
            "build",
            "--profile",
            WASM_PROFILE,
            "--target",
            WASM_TARGET,
            "-p",
            WASM_PACKAGE,
        ],
    )?;

    let build_root = workspace.join("target/wasm-sdk");
    let staging = build_root.join("package");
    let artifacts = build_root.join("artifacts");
    recreate_generated_dir(workspace, &staging)?;
    recreate_generated_dir(workspace, &artifacts)?;
    stage_package_sources(workspace, &staging)?;

    let internal = staging.join("internal");
    fs::create_dir_all(&internal)?;
    let wasm = cargo_target_root(workspace)
        .join(WASM_TARGET)
        .join(WASM_PROFILE)
        .join("continuity_wasm.wasm");
    let mut binding = Command::new(&environment.wasm_bindgen);
    binding
        .current_dir(workspace)
        .args(["--target", "web", "--out-dir"])
        .arg(&internal)
        .args(["--out-name", "continuity_wasm"])
        .arg(&wasm);
    require_success(&mut binding, "wasm-bindgen package generation")?;
    write_integrity_manifest(workspace, &staging)?;

    let mut npm_pack = Command::new(executable_name("npm"));
    configure_npm(workspace, &mut npm_pack);
    npm_pack
        .current_dir(workspace)
        .arg("pack")
        .arg(&staging)
        .arg("--pack-destination")
        .arg(&artifacts);
    require_success(&mut npm_pack, "npm pack")?;
    package_artifact::normalize_single_tarball(&staging, &artifacts)
}

fn stage_package_sources(workspace: &Path, staging: &Path) -> Result<()> {
    let package = workspace.join("packages/editor");
    for name in [
        "package.json",
        "index.js",
        "index.js.map",
        "index.d.ts",
        "react.js",
        "react.d.ts",
        "controller.js",
        "controller.d.ts",
        "lazy.js",
        "lazy.d.ts",
        "svelte.js",
        "svelte.d.ts",
        "vue.js",
        "vue.d.ts",
        "commit-queue.js",
        "commit-queue.d.ts",
        "conformance.js",
        "conformance.d.ts",
        "README.md",
        "CHANGELOG.md",
        "styles.css",
    ] {
        copy_file(&package.join(name), &staging.join(name))?;
    }
    copy_directory(&package.join("src"), &staging.join("src"))?;
    copy_file(&workspace.join("LICENSE"), &staging.join("LICENSE"))?;
    Ok(())
}

fn write_integrity_manifest(workspace: &Path, staging: &Path) -> Result<()> {
    let mut command = Command::new(executable_name("node"));
    command
        .current_dir(workspace)
        .arg("packages/editor/tools/write-integrity.mjs")
        .arg(staging);
    require_success(&mut command, "npm integrity metadata")
}

fn run_clean_consumer(workspace: &Path, tarball: &Path) -> Result<()> {
    let consumer = workspace.join("target/wasm-sdk/consumer");
    recreate_generated_dir(workspace, &consumer)?;
    fs::write(
        consumer.join("package.json"),
        "{\n  \"private\": true,\n  \"type\": \"module\"\n}\n",
    )?;
    copy_file(
        &workspace.join("packages/editor/tests/consumer.mjs"),
        &consumer.join("consumer.mjs"),
    )?;
    copy_file(
        &workspace.join("packages/editor/tests/controlled-controller.test.mjs"),
        &consumer.join("controlled-controller.test.mjs"),
    )?;
    copy_file(
        &workspace.join("packages/editor/tests/coordinates.test.mjs"),
        &consumer.join("coordinates.test.mjs"),
    )?;
    copy_file(
        &workspace.join("packages/editor/tests/source_lines.test.mjs"),
        &consumer.join("source_lines.test.mjs"),
    )?;
    copy_file(
        &workspace.join("packages/editor/tests/embedding-contracts.test.mjs"),
        &consumer.join("embedding-contracts.test.mjs"),
    )?;
    copy_file(
        &workspace.join("packages/editor/tests/composition-defer.test.mjs"),
        &consumer.join("composition-defer.test.mjs"),
    )?;
    copy_file(
        &workspace.join("packages/editor/tests/selection-actions-placement.test.mjs"),
        &consumer.join("selection-actions-placement.test.mjs"),
    )?;
    copy_file(
        &workspace.join("crates/test_fixtures/fixtures/wasm_engine_parity.json"),
        &consumer.join("wasm_engine_parity.json"),
    )?;

    let mut install = Command::new(executable_name("npm"));
    configure_npm(workspace, &mut install);
    install
        .current_dir(&consumer)
        .args(["install", "--ignore-scripts", "--no-audit", "--no-fund"])
        .arg(tarball);
    require_success(&mut install, "clean npm tarball install")?;

    let mut test = Command::new(executable_name("node"));
    test.current_dir(&consumer)
        .arg("consumer.mjs")
        .arg("wasm_engine_parity.json");
    require_success(&mut test, "packed npm consumer parity and budgets")?;

    consumer_tests::run_node_contracts(&consumer)
}

fn run_browser_consumer(workspace: &Path, tarball: &Path) -> Result<()> {
    let consumer = workspace.join("target/wasm-sdk/browser-consumer");
    recreate_generated_dir(workspace, &consumer)?;
    fs::write(
        consumer.join("package.json"),
        "{\n  \"private\": true,\n  \"type\": \"module\",\n  \"dependencies\": {\n    \"react\": \"18.3.1\",\n    \"react-dom\": \"18.3.1\",\n    \"vue\": \"3.5.17\"\n  },\n  \"devDependencies\": {\n    \"@types/react\": \"18.3.24\",\n    \"@types/react-dom\": \"18.3.7\",\n    \"typescript\": \"5.9.2\"\n  }\n}\n",
    )?;
    let tests = workspace.join("packages/editor/tests");
    for name in [
        "browser.html",
        "browser-niceties.mjs",
        "browser-performance.mjs",
        "browser-pointer-contract.mjs",
        "browser-profile.mjs",
        "browser-static-server.mjs",
        "react-browser.mjs",
        "react-browser-runtime.mjs",
        "react-consumer.tsx",
        "framework-consumer.ts",
        "react-tsconfig.json",
        "browser-suite.mjs",
        "browser-command-rail.mjs",
        "browser-rail-extensions.mjs",
        "browser-composition.mjs",
        "browser-composition-pointer.mjs",
        "browser-touch-pointer.mjs",
        "browser-touch-helpers.mjs",
        "browser-touch-selection.mjs",
        "browser-touch-scroll.mjs",
        "browser-touch-shield.mjs",
        "browser-soft-keyboard.mjs",
        "browser-coarse-pass.mjs",
        "browser-projection-chrome.mjs",
        "browser-shield-audit.mjs",
        "browser-viewport-persistence.mjs",
        "browser-mobile-ime.mjs",
        "browser-list-newline.mjs",
        "browser-viewport-lines.mjs",
        "browser-range-reveal.mjs",
        "browser-runner.mjs",
        "manual.html",
        "manual.mjs",
        "manual-server.mjs",
    ] {
        copy_file(&tests.join(name), &consumer.join(name))?;
    }
    install_tarball(workspace, &consumer, tarball, true)?;
    let mut type_test = Command::new(executable_name("node"));
    type_test
        .current_dir(&consumer)
        .arg(consumer.join("node_modules/typescript/bin/tsc"))
        .args(["--project", "react-tsconfig.json"]);
    require_success(&mut type_test, "packed React TypeScript consumer")?;
    let mut test = Command::new(executable_name("node"));
    test.current_dir(&consumer)
        .arg("browser-runner.mjs")
        .arg(&consumer);
    require_success(
        &mut test,
        "packed Web Component Chromium contract and budgets",
    )
}

fn run_electron_consumer(workspace: &Path, tarball: &Path) -> Result<()> {
    let consumer = workspace.join("target/wasm-sdk/electron-consumer");
    recreate_generated_dir(workspace, &consumer)?;
    let example = workspace.join("apps/electron-example");
    for name in [
        "main.mjs",
        "preload.cjs",
        "renderer.mjs",
        "index.html",
        "README.md",
    ] {
        copy_file(&example.join(name), &consumer.join(name))?;
    }
    let package = serde_json::json!({
        "name": "continuity-electron-packed-smoke",
        "private": true,
        "type": "module",
        "main": "main.mjs",
        "scripts": {
            "start": "electron .",
            "smoke": "electron . --smoke"
        },
        "devDependencies": { "electron": "43.1.1" }
    });
    fs::write(
        consumer.join("package.json"),
        format!("{}\n", serde_json::to_string_pretty(&package)?),
    )?;
    install_tarball(workspace, &consumer, tarball, false)?;

    let user_data = consumer.join("user-data");
    fs::create_dir_all(&user_data)?;
    let mut command = electron_smoke_command();
    configure_npm(workspace, &mut command);
    command
        .current_dir(&consumer)
        .env("CONTINUITY_ELECTRON_SMOKE", "1")
        .env("CONTINUITY_ELECTRON_USER_DATA", &user_data)
        .env("ELECTRON_DISABLE_SECURITY_WARNINGS", "1");
    require_success(&mut command, "packed Electron IPC persistence smoke")?;
    require_success(&mut command, "packed Electron restart persistence smoke")
}

fn install_tarball(
    workspace: &Path,
    consumer: &Path,
    tarball: &Path,
    ignore_scripts: bool,
) -> Result<()> {
    let mut install = Command::new(executable_name("npm"));
    configure_npm(workspace, &mut install);
    install
        .current_dir(consumer)
        .args(["install", "--no-audit", "--no-fund"]);
    if ignore_scripts {
        install.arg("--ignore-scripts");
    }
    install.arg(tarball);
    require_success(&mut install, "clean packed npm install")
}

fn electron_smoke_command() -> Command {
    #[cfg(target_os = "linux")]
    {
        let mut command = Command::new("dbus-run-session");
        command.args([
            "--",
            "xvfb-run",
            "-a",
            "npm",
            "run",
            "smoke",
            "--",
            "--no-sandbox",
        ]);
        command
    }
    #[cfg(not(target_os = "linux"))]
    {
        let mut command = Command::new(executable_name("npm"));
        command.args(["run", "smoke"]);
        command
    }
}

fn run_portable_cargo(
    workspace: &Path,
    environment: &WasmBuildEnvironment,
    args: &[&str],
) -> Result<()> {
    let mut command = Command::new(env!("CARGO"));
    command.current_dir(workspace).args(args);
    environment.apply(&mut command);
    require_success(&mut command, &format!("cargo {}", args.join(" ")))
}

fn run_cargo(workspace: &Path, args: &[&str]) -> Result<()> {
    let mut command = Command::new(env!("CARGO"));
    command.current_dir(workspace).args(args);
    require_success(&mut command, &format!("cargo {}", args.join(" ")))
}

fn require_success(command: &mut Command, label: &str) -> Result<()> {
    eprintln!("wasm: {label}");
    let status = command
        .status()
        .with_context(|| format!("starting {label}"))?;
    if !status.success() {
        bail!("{label} failed (exit {:?})", status.code());
    }
    Ok(())
}

fn cargo_target_root(workspace: &Path) -> PathBuf {
    env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                workspace.join(path)
            }
        })
        .unwrap_or_else(|| workspace.join("target"))
}

fn copy_file(source: &Path, destination: &Path) -> Result<()> {
    fs::copy(source, destination)
        .with_context(|| format!("copying {} to {}", source.display(), destination.display()))?;
    Ok(())
}

fn copy_directory(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_directory(&source_path, &destination_path)?;
        } else {
            copy_file(&source_path, &destination_path)?;
        }
    }
    Ok(())
}

fn executable_name(base: &str) -> OsString {
    if cfg!(windows) && matches!(base, "npm") {
        OsString::from(format!("{base}.cmd"))
    } else if cfg!(windows) {
        OsString::from(format!("{base}.exe"))
    } else {
        OsString::from(base)
    }
}

fn configure_npm(workspace: &Path, command: &mut Command) {
    command.env(
        "NPM_CONFIG_CACHE",
        workspace.join("target/wasm-sdk/npm-cache"),
    );
    command.env("NPM_CONFIG_UPDATE_NOTIFIER", "false");
}
