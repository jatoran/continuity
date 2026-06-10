//! Phase 18 release-artifact tasks.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};

pub(crate) const APP_PACKAGE: &str = "continuity-app";
pub(crate) const EXE_NAME: &str = "continuity.exe";
pub(crate) const PORTABLE_ZIP: &str = "continuity-portable.zip";
const RELEASE_PROFILE: &str = "release-small";
const BINARY_SIZE_BUDGET_BYTES: u64 = 9 * 1024 * 1024;

/// Build the release executable and assemble portable artifacts.
pub(crate) fn package() -> Result<()> {
    build_release_binary()?;
    let root = workspace_root();
    let artifacts = release_artifacts_dir();
    recreate_dir(&artifacts)?;
    copy_uninstaller_helpers(&root, &artifacts)?;

    let exe = release_binary_path()?;
    check_binary_size(&exe)?;
    fs::copy(&exe, artifacts.join(EXE_NAME))
        .with_context(|| format!("copying {}", exe.display()))?;
    fs::copy(
        root.join("crates/app/assets/continuity.ico"),
        artifacts.join("continuity.ico"),
    )?;

    let portable_root = artifacts.join("continuity-portable");
    recreate_dir(&portable_root)?;
    fs::copy(&exe, portable_root.join(EXE_NAME))
        .with_context(|| format!("copying portable {}", exe.display()))?;
    let data_dir = portable_root.join("data");
    let themes_dir = data_dir.join("themes");
    fs::create_dir_all(&themes_dir)?;
    fs::write(data_dir.join("settings.toml"), default_settings_toml())?;
    fs::copy(
        root.join("crates/keymap/assets/default.toml"),
        data_dir.join("keymap.toml"),
    )?;
    copy_dir(&root.join("crates/theme/assets"), &themes_dir, |path| {
        path.extension().is_some_and(|ext| ext == "toml")
    })?;

    let zip_path = artifacts.join(PORTABLE_ZIP);
    if zip_path.exists() {
        fs::remove_file(&zip_path)?;
    }
    compress_zip(&portable_root, &zip_path)?;
    eprintln!("release artifacts written to {}", artifacts.display());
    Ok(())
}

/// Sign the release executable using the configured code-signing cert.
pub(crate) fn sign() -> Result<()> {
    let exe = release_binary_path()?;
    sign_path(&exe)?;
    refresh_signed_artifacts(&exe)?;
    let installer = release_artifacts_dir().join(crate::installer::INSTALLER_MSI);
    if installer.exists() {
        let rebuilt_installer = crate::installer::build_from_existing_package()?;
        sign_path(&rebuilt_installer)?;
    }
    Ok(())
}

/// Build, sign, and package release artifacts.
pub(crate) fn release(args: &[String]) -> Result<()> {
    let skip_sign = args.iter().any(|arg| arg == "--skip-sign");
    package()?;
    if skip_sign {
        eprintln!("release: skipping code signing because --skip-sign was passed");
    } else {
        sign()?;
    }
    let installer = crate::installer::build_from_existing_package()?;
    if !skip_sign {
        sign_path(&installer)?;
    }
    Ok(())
}

fn build_release_binary() -> Result<()> {
    let status = Command::new(env!("CARGO"))
        .args(["build", "--profile", RELEASE_PROFILE, "-p", APP_PACKAGE])
        .status()?;
    if !status.success() {
        bail!("cargo build --profile {RELEASE_PROFILE} -p {APP_PACKAGE} failed");
    }
    Ok(())
}

fn check_binary_size(path: &Path) -> Result<()> {
    let bytes = fs::metadata(path)?.len();
    if bytes > BINARY_SIZE_BUDGET_BYTES {
        bail!(
            "{} is {} bytes, budget {} bytes",
            path.display(),
            bytes,
            BINARY_SIZE_BUDGET_BYTES
        );
    }
    Ok(())
}

fn refresh_signed_artifacts(signed_exe: &Path) -> Result<()> {
    let artifacts = release_artifacts_dir();
    if !artifacts.exists() {
        return Ok(());
    }
    let artifact_exe = artifacts.join(EXE_NAME);
    if artifact_exe.exists() {
        fs::copy(signed_exe, &artifact_exe)
            .with_context(|| format!("copying signed {}", artifact_exe.display()))?;
    }
    let portable_root = artifacts.join("continuity-portable");
    let portable_exe = portable_root.join(EXE_NAME);
    if portable_exe.exists() {
        fs::copy(signed_exe, &portable_exe)
            .with_context(|| format!("copying signed {}", portable_exe.display()))?;
        let zip_path = artifacts.join(PORTABLE_ZIP);
        if zip_path.exists() {
            fs::remove_file(&zip_path)?;
        }
        compress_zip(&portable_root, &zip_path)?;
    }
    Ok(())
}

pub(crate) fn sign_path(path: &Path) -> Result<()> {
    let cert = env::var("CONTINUITY_SIGN_CERT")
        .context("CONTINUITY_SIGN_CERT must point at the signing certificate")?;
    let pass = env::var("CONTINUITY_SIGN_PASS")
        .context("CONTINUITY_SIGN_PASS must contain the certificate password")?;
    let signtool = env::var("CONTINUITY_SIGNTOOL").unwrap_or_else(|_| "signtool.exe".into());
    let status = Command::new(signtool)
        .args([
            "sign",
            "/fd",
            "SHA256",
            "/f",
            &cert,
            "/p",
            &pass,
            "/tr",
            "http://timestamp.digicert.com",
            "/td",
            "SHA256",
        ])
        .arg(path)
        .status()?;
    if !status.success() {
        bail!("signtool failed for {}", path.display());
    }
    Ok(())
}

fn compress_zip(source_dir: &Path, zip_path: &Path) -> Result<()> {
    let source = format!("{}\\*", escape_powershell_path(source_dir));
    let destination = escape_powershell_path(zip_path);
    let script =
        format!("Compress-Archive -Path '{source}' -DestinationPath '{destination}' -Force");
    let status = Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .status()?;
    if !status.success() {
        bail!("Compress-Archive failed for {}", zip_path.display());
    }
    Ok(())
}

fn escape_powershell_path(path: &Path) -> String {
    path.display().to_string().replace('\'', "''")
}

fn copy_dir(source: &Path, destination: &Path, include: impl Fn(&Path) -> bool) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && include(&path) {
            fs::copy(&path, destination.join(entry.file_name()))?;
        }
    }
    Ok(())
}

fn copy_uninstaller_helpers(root: &Path, artifacts: &Path) -> Result<()> {
    for name in ["uninstall-continuity.cmd", "uninstall-continuity.ps1"] {
        let source = root.join("installer").join(name);
        let destination = artifacts.join(name);
        fs::copy(&source, &destination).with_context(|| format!("copying {}", source.display()))?;
    }
    Ok(())
}

fn recreate_dir(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path)?;
    }
    fs::create_dir_all(path)?;
    Ok(())
}

pub(crate) fn release_artifacts_dir() -> PathBuf {
    workspace_root().join("target").join("release-artifacts")
}

fn release_binary_path() -> Result<PathBuf> {
    let target = workspace_root().join("target");
    let candidates = [
        target.join(RELEASE_PROFILE).join(EXE_NAME),
        target
            .join("x86_64-pc-windows-msvc")
            .join(RELEASE_PROFILE)
            .join(EXE_NAME),
    ];
    candidates
        .into_iter()
        .find(|path| path.exists())
        .ok_or_else(|| anyhow!("release binary not found; run cargo xtask package first"))
}

pub(crate) fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn default_settings_toml() -> &'static str {
    r#"# Continuity settings.
# Empty or partial files are valid; omitted keys use built-in defaults.

[persistence]
mode = "balanced"

[editor]
font_family_prose = "Segoe UI Variable"
font_family_mono = "Cascadia Mono"
word_wrap = true

[ui]
theme = "system"
theme_dark = "deep_minimal"
theme_light = "paper"
"#
}
