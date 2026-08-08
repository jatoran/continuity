//! Immutable SDK release manifest, checksums, and bundle verification.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(crate) const RELEASE_RECORD_NAME: &str = "release-manifest.json";
pub(crate) const CHECKSUM_NAME: &str = "SHA256SUMS.txt";

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReleaseRecord {
    pub(crate) schema_version: u32,
    pub(crate) sdk_version: String,
    pub(crate) channel: String,
    pub(crate) tag: String,
    pub(crate) source_commit: String,
    pub(crate) source_dirty: bool,
    pub(crate) generated_unix_seconds: u64,
    pub(crate) artifacts: Vec<ArtifactRecord>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtifactRecord {
    pub(crate) path: String,
    pub(crate) bytes: u64,
    pub(crate) sha256: String,
}

pub(crate) fn write_release_record(
    directory: &Path,
    sdk_version: &str,
    channel: &str,
    tag: &str,
    source_commit: &str,
    source_dirty: bool,
    generated_unix_seconds: u64,
) -> Result<PathBuf> {
    let artifacts = collect_artifacts(directory)?;
    let checksum_path = directory.join(CHECKSUM_NAME);
    let mut checksum = File::create(&checksum_path)
        .with_context(|| format!("create {}", checksum_path.display()))?;
    for artifact in &artifacts {
        writeln!(checksum, "{}  {}", artifact.sha256, artifact.path)?;
    }
    let record = ReleaseRecord {
        schema_version: 1,
        sdk_version: sdk_version.to_owned(),
        channel: channel.to_owned(),
        tag: tag.to_owned(),
        source_commit: source_commit.to_owned(),
        source_dirty,
        generated_unix_seconds,
        artifacts,
    };
    let path = directory.join(RELEASE_RECORD_NAME);
    let serialized = serde_json::to_vec_pretty(&record)?;
    fs::write(&path, serialized).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

pub(crate) fn verify_release_directory(
    directory: &Path,
    expected_version: &str,
) -> Result<ReleaseRecord> {
    let record_path = directory.join(RELEASE_RECORD_NAME);
    let record: ReleaseRecord = serde_json::from_slice(
        &fs::read(&record_path).with_context(|| format!("read {}", record_path.display()))?,
    )?;
    if record.schema_version != 1 {
        bail!(
            "unsupported SDK release manifest schema {}",
            record.schema_version
        );
    }
    if record.sdk_version != expected_version {
        bail!(
            "release bundle version {} does not match canonical version {expected_version}",
            record.sdk_version
        );
    }
    if record.artifacts.is_empty() {
        bail!("release bundle contains no artifacts");
    }
    for artifact in &record.artifacts {
        validate_relative_name(&artifact.path)?;
        let path = directory.join(&artifact.path);
        let metadata = fs::metadata(&path)
            .with_context(|| format!("release artifact {} is missing", path.display()))?;
        if metadata.len() != artifact.bytes {
            bail!("release artifact {} byte length changed", artifact.path);
        }
        let actual = sha256(&path)?;
        if actual != artifact.sha256 {
            bail!("release artifact {} checksum changed", artifact.path);
        }
    }
    verify_checksum_file(directory, &record.artifacts)?;
    Ok(record)
}

fn collect_artifacts(directory: &Path) -> Result<Vec<ArtifactRecord>> {
    let mut paths: Vec<PathBuf> = fs::read_dir(directory)
        .with_context(|| format!("read release directory {}", directory.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| {
            path.file_name().and_then(|name| name.to_str()) != Some(RELEASE_RECORD_NAME)
                && path.file_name().and_then(|name| name.to_str()) != Some(CHECKSUM_NAME)
        })
        .collect();
    paths.sort();
    paths
        .iter()
        .map(|path| {
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| anyhow!("release artifact name is not UTF-8: {}", path.display()))?;
            validate_relative_name(name)?;
            Ok(ArtifactRecord {
                path: name.to_owned(),
                bytes: fs::metadata(path)?.len(),
                sha256: sha256(path)?,
            })
        })
        .collect()
}

fn verify_checksum_file(directory: &Path, artifacts: &[ArtifactRecord]) -> Result<()> {
    let expected = artifacts
        .iter()
        .map(|artifact| format!("{}  {}\n", artifact.sha256, artifact.path))
        .collect::<String>();
    let path = directory.join(CHECKSUM_NAME);
    let actual = fs::read_to_string(&path)
        .with_context(|| format!("read checksum manifest {}", path.display()))?;
    if actual.replace("\r\n", "\n") != expected {
        bail!("{} does not match the release manifest", path.display());
    }
    Ok(())
}

fn validate_relative_name(name: &str) -> Result<()> {
    let path = Path::new(name);
    if name.is_empty()
        || path.is_absolute()
        || path.components().count() != 1
        || name == "."
        || name == ".."
    {
        bail!("unsafe release artifact path `{name}`");
    }
    Ok(())
}

fn sha256(path: &Path) -> Result<String> {
    let mut input = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{verify_release_directory, write_release_record};

    #[test]
    fn release_bundle_detects_artifact_mutation() {
        let directory = tempdir().expect("invariant: tempdir");
        fs::write(directory.path().join("engine.crate"), b"accepted")
            .expect("invariant: artifact write");
        write_release_record(
            directory.path(),
            "0.1.0",
            "preview",
            "sdk-v0.1.0",
            "abc",
            false,
            1,
        )
        .expect("invariant: manifest write");
        verify_release_directory(directory.path(), "0.1.0")
            .expect("invariant: initial verification");
        fs::write(directory.path().join("engine.crate"), b"changed")
            .expect("invariant: artifact mutation");
        assert!(verify_release_directory(directory.path(), "0.1.0").is_err());
    }
}
