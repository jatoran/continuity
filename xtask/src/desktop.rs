//! Cross-platform desktop package and artifact validation.

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;

const STARTUP_BUDGET_MS: f64 = 4_000.0;
const WORKING_SET_BUDGET_BYTES: u64 = 512 * 1024 * 1024;
const UNPACKED_BUDGET_BYTES: u64 = 450 * 1024 * 1024;
const DISTRIBUTABLE_BUDGET_BYTES: u64 = 200 * 1024 * 1024;
const ASAR_BUDGET_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DesktopMetrics {
    platform: String,
    run: u8,
    startup_ms: f64,
    working_set_bytes: u64,
    revision: u64,
    durable_sequence: u64,
}

/// Build the packed Web Component, package the desktop shell, and smoke its artifact.
pub(crate) fn check() -> Result<()> {
    let workspace = crate::release::workspace_root();
    let app_dir = workspace.join("apps/desktop-web");
    let tarball = crate::wasm::package_tarball()?;
    install_dependencies(&workspace, &app_dir, &tarball)?;
    run_npm(&workspace, &app_dir, &["test"])?;
    run_npm(
        &workspace,
        &app_dir,
        &["audit", "--omit=dev", "--audit-level=high"],
    )?;

    let output_dir = workspace.join("target/desktop-web").join(format!(
        "out-{}-{}",
        env::consts::OS,
        std::process::id()
    ));
    fs::create_dir_all(
        output_dir
            .parent()
            .context("desktop output directory has no parent")?,
    )?;
    let mut make = npm_command();
    configure_npm(&workspace, &mut make);
    make.current_dir(&app_dir)
        .env("CONTINUITY_DESKTOP_OUT_DIR", &output_dir)
        .args(["run", "make"]);
    require_success(&mut make, "Electron Forge make")?;
    fs::write(
        workspace.join("target/desktop-web/latest-output.txt"),
        output_dir.to_string_lossy().as_bytes(),
    )?;

    let package_dir = packaged_application_dir(&output_dir);
    enforce_artifact_budgets(&package_dir, &output_dir.join("make"))?;
    audit_asar(&app_dir, &package_dir)?;
    let executable = packaged_executable(&package_dir);
    let user_data = output_dir.join("smoke-user-data");
    let first = run_smoke(&executable, &user_data, 1)?;
    let second = run_smoke(&executable, &user_data, 2)?;
    enforce_runtime_budgets(&first, &second)?;
    run_single_instance_probe(&executable, &output_dir)?;
    run_close_probe(&executable, &output_dir)?;

    eprintln!(
        "desktop-check: PASS ({}, startup {:.1}/{:.1} ms, memory {}/{}, output {})",
        first.platform,
        first.startup_ms,
        second.startup_ms,
        first.working_set_bytes,
        second.working_set_bytes,
        output_dir.display()
    );
    Ok(())
}

fn run_close_probe(executable: &Path, output_dir: &Path) -> Result<()> {
    let mut command = smoke_command(executable);
    command
        .env(
            "CONTINUITY_DESKTOP_USER_DATA",
            output_dir.join("close-probe-user-data"),
        )
        .env("CONTINUITY_DESKTOP_CLOSE_PROBE", "1");
    let output = command
        .output()
        .context("launching packaged desktop close handshake probe")?;
    emit_output(&output);
    if !output.status.success() {
        bail!(
            "desktop close handshake probe failed with {}",
            output.status
        );
    }
    if !String::from_utf8_lossy(&output.stdout).contains("CONTINUITY_DESKTOP_CLOSE PASS") {
        bail!("desktop close handshake probe did not confirm final-edit durability");
    }
    Ok(())
}

fn run_single_instance_probe(executable: &Path, output_dir: &Path) -> Result<()> {
    let probe_dir = output_dir.join("single-instance-probe");
    let user_data = output_dir.join("single-instance-user-data");
    let document = probe_dir.join("forwarded.md");
    fs::create_dir_all(&probe_dir)?;
    fs::write(&document, "# Forwarded\n")?;

    let mut primary_command = smoke_command(executable);
    primary_command
        .env("CONTINUITY_DESKTOP_USER_DATA", &user_data)
        .env("CONTINUITY_DESKTOP_INSTANCE_PROBE", &probe_dir);
    let mut primary = primary_command
        .spawn()
        .context("launching desktop single-instance primary")?;
    let outcome = (|| -> Result<()> {
        wait_for_marker(
            &probe_dir.join("primary-ready"),
            &mut primary,
            "primary readiness",
        )?;

        let mut secondary = smoke_command(executable);
        secondary
            .arg(&document)
            .env("CONTINUITY_DESKTOP_USER_DATA", &user_data)
            .env("CONTINUITY_DESKTOP_INSTANCE_PROBE", &probe_dir);
        let output = secondary
            .output()
            .context("launching desktop single-instance secondary")?;
        emit_output(&output);
        if !output.status.success() {
            bail!(
                "desktop single-instance secondary failed with {}",
                output.status
            );
        }

        let received = probe_dir.join("second-received.json");
        wait_for_marker(&received, &mut primary, "secondary handoff")?;
        // `writeFile` creates the marker before its payload is fully visible.
        // The primary exits only after that promise resolves, so wait for exit
        // before parsing instead of racing a partially written JSON file.
        wait_for_exit(&mut primary, "single-instance primary shutdown")?;
        let arguments: Vec<String> = serde_json::from_slice(&fs::read(&received)?)?;
        if !arguments
            .iter()
            .any(|argument| Path::new(argument) == document)
        {
            bail!("desktop second-instance handoff omitted the document path");
        }
        Ok(())
    })();
    if outcome.is_err() {
        let _ = primary.kill();
    }
    let _ = primary.wait();
    outcome
}

fn wait_for_marker(path: &Path, child: &mut Child, label: &str) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if path.is_file() {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            bail!("{label} process exited early with {status}");
        }
        thread::sleep(Duration::from_millis(50));
    }
    bail!("timed out waiting for {label}: {}", path.display())
}

fn wait_for_exit(child: &mut Child, label: &str) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait()? {
            if status.success() {
                return Ok(());
            }
            bail!("{label} failed with {status}");
        }
        thread::sleep(Duration::from_millis(50));
    }
    bail!("timed out waiting for {label}")
}

fn install_dependencies(workspace: &Path, app_dir: &Path, tarball: &Path) -> Result<()> {
    let attempts = if cfg!(windows) { 3 } else { 1 };
    for attempt in 1..=attempts {
        let mut install = npm_command();
        configure_npm(workspace, &mut install);
        install.current_dir(app_dir).arg("ci");
        let status = install.status().context("starting npm ci")?;
        if status.success() {
            break;
        }
        if attempt == attempts {
            bail!("npm ci failed with exit code: {status}");
        }
        eprintln!(
            "desktop-check: npm ci attempt {attempt} failed; retrying for a transient Windows lock"
        );
        thread::sleep(Duration::from_millis(500));
    }
    let mut install = npm_command();
    configure_npm(workspace, &mut install);
    install
        .current_dir(app_dir)
        .args(["install", "--no-save"])
        .arg(tarball);
    require_success(&mut install, "packed editor installation")
}

fn run_npm(workspace: &Path, directory: &Path, args: &[&str]) -> Result<()> {
    let mut command = npm_command();
    configure_npm(workspace, &mut command);
    command.current_dir(directory).args(args);
    require_success(&mut command, &format!("npm {}", args.join(" ")))
}

fn configure_npm(workspace: &Path, command: &mut Command) {
    command
        .env("NPM_CONFIG_CACHE", workspace.join("target/npm-cache"))
        .env("NPM_CONFIG_FUND", "false")
        .env("NPM_CONFIG_UPDATE_NOTIFIER", "false");
}

fn require_success(command: &mut Command, label: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("running {label}"))?;
    if !status.success() {
        bail!("{label} failed with {status}");
    }
    Ok(())
}

fn packaged_application_dir(output_dir: &Path) -> PathBuf {
    output_dir.join(format!(
        "Continuity Web-{}-{}",
        electron_platform(),
        electron_arch()
    ))
}

fn packaged_executable(package_dir: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    return package_dir.join("continuity-web.exe");
    #[cfg(target_os = "macos")]
    return package_dir.join("Continuity Web.app/Contents/MacOS/continuity-web");
    #[cfg(target_os = "linux")]
    return package_dir.join("continuity-web");
}

fn run_smoke(executable: &Path, user_data: &Path, run: u8) -> Result<DesktopMetrics> {
    if !executable.is_file() {
        bail!("packaged executable is missing: {}", executable.display());
    }
    let mut command = smoke_command(executable);
    command.arg("--smoke");
    command
        .env("CONTINUITY_DESKTOP_USER_DATA", user_data)
        .env("CONTINUITY_DESKTOP_SMOKE_RUN", run.to_string());
    let output = command
        .output()
        .with_context(|| format!("launching packaged desktop smoke {run}"))?;
    emit_output(&output);
    if !output.status.success() {
        bail!("packaged desktop smoke {run} failed with {}", output.status);
    }
    parse_metrics(&output.stdout)
}

fn smoke_command(executable: &Path) -> Command {
    #[cfg(target_os = "linux")]
    {
        let mut command = Command::new("dbus-run-session");
        command.args(["--", "xvfb-run", "-a"]);
        command.arg(executable);
        command.arg("--no-sandbox");
        command
    }
    #[cfg(not(target_os = "linux"))]
    {
        Command::new(executable)
    }
}

fn emit_output(output: &Output) {
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    print!("{}", String::from_utf8_lossy(&output.stdout));
}

fn parse_metrics(stdout: &[u8]) -> Result<DesktopMetrics> {
    let text = String::from_utf8_lossy(stdout);
    let payload = text
        .lines()
        .find_map(|line| line.strip_prefix("CONTINUITY_DESKTOP_METRICS "))
        .ok_or_else(|| anyhow!("desktop smoke did not emit metrics"))?;
    serde_json::from_str(payload).context("parsing desktop smoke metrics")
}

fn enforce_runtime_budgets(first: &DesktopMetrics, second: &DesktopMetrics) -> Result<()> {
    for metrics in [first, second] {
        if metrics.startup_ms > STARTUP_BUDGET_MS {
            bail!(
                "desktop startup {:.1} ms exceeded {:.1} ms",
                metrics.startup_ms,
                STARTUP_BUDGET_MS
            );
        }
        if metrics.working_set_bytes > WORKING_SET_BUDGET_BYTES {
            bail!(
                "desktop working set {} exceeded {} bytes",
                metrics.working_set_bytes,
                WORKING_SET_BUDGET_BYTES
            );
        }
    }
    if first.run != 1
        || first.revision != 1
        || first.durable_sequence != 2
        || second.run != 2
        || second.revision != 2
        || second.durable_sequence != 4
    {
        bail!("desktop restart did not preserve revision and durable sequence continuity");
    }
    Ok(())
}

fn enforce_artifact_budgets(package_dir: &Path, make_dir: &Path) -> Result<()> {
    let unpacked = directory_size(package_dir)?;
    if unpacked > UNPACKED_BUDGET_BYTES {
        bail!("desktop package {unpacked} exceeded {UNPACKED_BUDGET_BYTES} bytes");
    }
    let required = required_distributable_extensions();
    let artifacts = collect_files(make_dir)?;
    for extension in required {
        if !artifacts.iter().any(|path| has_extension(path, extension)) {
            bail!("desktop maker did not produce a .{extension} artifact");
        }
    }
    for artifact in artifacts {
        let bytes = fs::metadata(&artifact)?.len();
        if bytes > DISTRIBUTABLE_BUDGET_BYTES {
            bail!(
                "desktop artifact {} is {bytes} bytes; budget is {DISTRIBUTABLE_BUDGET_BYTES}",
                artifact.display()
            );
        }
    }
    Ok(())
}

fn audit_asar(app_dir: &Path, package_dir: &Path) -> Result<()> {
    let asar = collect_files(package_dir)?
        .into_iter()
        .find(|path| path.file_name() == Some(OsStr::new("app.asar")))
        .ok_or_else(|| anyhow!("packaged app.asar is missing"))?;
    let bytes = fs::metadata(&asar)?.len();
    if bytes > ASAR_BUDGET_BYTES {
        bail!("desktop app.asar {bytes} exceeded {ASAR_BUDGET_BYTES} bytes");
    }
    let output = Command::new(node_executable())
        .current_dir(app_dir)
        .arg("node_modules/@electron/asar/bin/asar.js")
        .arg("list")
        .arg(&asar)
        .output()
        .context("listing packaged ASAR")?;
    if !output.status.success() {
        bail!("ASAR inventory failed with {}", output.status);
    }
    let inventory = String::from_utf8_lossy(&output.stdout).replace('\\', "/");
    for required in [
        "/node_modules/@continuity-editor/editor/internal/continuity_wasm.js",
        "/node_modules/@continuity-editor/editor/internal/continuity_wasm_bg.wasm",
    ] {
        if !inventory.contains(required) {
            bail!("ASAR is missing packed editor entry {required}");
        }
    }
    for forbidden in ["/tests/", "/package-lock.json", "/out-"] {
        if inventory.contains(forbidden) {
            bail!("ASAR contains forbidden development entry {forbidden}");
        }
    }
    Ok(())
}

fn directory_size(path: &Path) -> Result<u64> {
    collect_files(path)?
        .into_iter()
        .try_fold(0_u64, |sum, file| {
            Ok(sum.saturating_add(fs::metadata(file)?.len()))
        })
}

fn collect_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in
            fs::read_dir(&directory).with_context(|| format!("reading {}", directory.display()))?
        {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                directories.push(entry.path());
            } else if file_type.is_file() {
                files.push(entry.path());
            }
        }
    }
    Ok(files)
}

fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension().and_then(OsStr::to_str) == Some(extension)
}

#[cfg(target_os = "windows")]
fn required_distributable_extensions() -> &'static [&'static str] {
    &["exe", "zip"]
}

#[cfg(target_os = "macos")]
fn required_distributable_extensions() -> &'static [&'static str] {
    &["dmg", "zip"]
}

#[cfg(target_os = "linux")]
fn required_distributable_extensions() -> &'static [&'static str] {
    &["deb", "zip"]
}

fn electron_platform() -> &'static str {
    match env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    }
}

fn electron_arch() -> &'static str {
    match env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    }
}

#[cfg(windows)]
fn npm_command() -> Command {
    Command::new("npm.cmd")
}

#[cfg(not(windows))]
fn npm_command() -> Command {
    Command::new("npm")
}

#[cfg(windows)]
fn node_executable() -> &'static str {
    "node.exe"
}

#[cfg(not(windows))]
fn node_executable() -> &'static str {
    "node"
}
