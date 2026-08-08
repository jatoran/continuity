//! npm pack artifact discovery and release-facing name normalization.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

pub(super) fn normalize_single_tarball(staging: &Path, artifacts: &Path) -> Result<PathBuf> {
    let tarball = find_single_tarball(artifacts)?;
    let manifest_text = fs::read_to_string(staging.join("package.json"))?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest_text)?;
    let version = manifest
        .get("version")
        .and_then(serde_json::Value::as_str)
        .context("npm package version is missing")?;
    let normalized = artifacts.join(format!("continuity-editor-{version}.tgz"));
    if tarball != normalized {
        fs::rename(tarball, &normalized)?;
    }
    Ok(normalized)
}

fn find_single_tarball(artifacts: &Path) -> Result<PathBuf> {
    let tarballs: Vec<PathBuf> = fs::read_dir(artifacts)?
        .filter_map(|entry| entry.ok().map(|value| value.path()))
        .filter(|path| path.extension() == Some(OsStr::new("tgz")))
        .collect();
    match tarballs.as_slice() {
        [tarball] => Ok(tarball.clone()),
        _ => bail!(
            "expected one npm tarball in {}, found {}",
            artifacts.display(),
            tarballs.len()
        ),
    }
}
