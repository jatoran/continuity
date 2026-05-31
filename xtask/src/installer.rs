//! Windows installer assembly.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::release;

pub(crate) const INSTALLER_MSI: &str = "continuity-setup.msi";

/// Build the portable artifacts and assemble the MSI installer.
pub(crate) fn installer() -> Result<()> {
    release::package()?;
    build_from_existing_package()?;
    Ok(())
}

/// Assemble the MSI from the already-staged release artifacts.
pub(crate) fn build_from_existing_package() -> Result<PathBuf> {
    let artifacts = release::release_artifacts_dir();
    let exe = artifacts.join(release::EXE_NAME);
    if !exe.exists() {
        bail!("{} missing; run cargo xtask package first", exe.display());
    }

    let root = release::workspace_root();
    let source = root.join("installer").join("continuity.wxs");
    let msi = artifacts.join(INSTALLER_MSI);
    let wix = env::var("CONTINUITY_WIX").unwrap_or_else(|_| "wix.exe".to_string());
    let eula = env::var("CONTINUITY_WIX_ACCEPT_EULA").unwrap_or_else(|_| "wix7".to_string());
    let artifact_dir = format!("ArtifactDir={}", artifacts.display());
    let product_version = format!("ProductVersion={}", app_product_version()?);
    let status = Command::new(&wix)
        .args([
            "build",
            "-acceptEula",
            &eula,
            "-arch",
            "x64",
            "-ext",
            "WixToolset.UI.wixext",
            "-d",
            &artifact_dir,
            "-d",
            &product_version,
            "-o",
        ])
        .arg(&msi)
        .arg(&source)
        .status()
        .with_context(|| format!("running WiX tool `{wix}`"))?;
    if !status.success() {
        bail!(
            "WiX installer build failed for {} (install WiX v7 + WixToolset.UI.wixext or set CONTINUITY_WIX)",
            source.display()
        );
    }
    eprintln!("installer artifact written to {}", msi.display());
    Ok(msi)
}

fn app_product_version() -> Result<String> {
    let manifest = release::workspace_root()
        .join("crates")
        .join("app")
        .join("Cargo.toml");
    let source =
        fs::read_to_string(&manifest).with_context(|| format!("reading {}", manifest.display()))?;
    let value: toml::Value = source
        .parse()
        .with_context(|| format!("parsing {}", manifest.display()))?;
    let version = value
        .get("package")
        .and_then(|package| package.get("version"))
        .and_then(toml::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("{} has no package.version", manifest.display()))?;
    compute_msi_product_version(version)
}

fn compute_msi_product_version(package_version: &str) -> Result<String> {
    let mut parts = package_version.split('.');
    let major = parse_msi_version_part(parts.next(), "major")?;
    let minor = parse_msi_version_part(parts.next(), "minor")?;
    let patch = parse_msi_version_part(parts.next(), "patch")?;
    if parts.next().is_some() {
        bail!("app version `{package_version}` has more than three version fields");
    }
    Ok(format!("{major}.{minor}.{patch}"))
}

fn parse_msi_version_part(part: Option<&str>, name: &str) -> Result<u16> {
    let part = part.ok_or_else(|| anyhow::anyhow!("app version is missing {name} field"))?;
    if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
        bail!("app version {name} field `{part}` is not numeric");
    }
    let value: u16 = part
        .parse()
        .with_context(|| format!("parsing app version {name} field `{part}`"))?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn installer_source_path_points_at_wix_file() {
        let path = release::workspace_root()
            .join("installer")
            .join("continuity.wxs");
        assert!(Path::new(&path).exists());
    }

    #[test]
    fn app_version_is_usable_as_msi_product_version() {
        assert_eq!(compute_msi_product_version("1.2.3").unwrap(), "1.2.3");
    }

    #[test]
    fn app_version_rejects_prerelease_for_msi_product_version() {
        let err = compute_msi_product_version("1.2.3-beta").unwrap_err();
        assert!(err.to_string().contains("not numeric"));
    }
}
