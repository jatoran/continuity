//! CycloneDX inventory for the Rust, npm, and Python SDK release closure.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::sdk_release_manifest::ReleaseConfig;

pub(crate) fn write(root: &Path, output: &Path, config: &ReleaseConfig) -> Result<()> {
    let metadata = cargo_metadata(root)?;
    let packages = metadata["packages"]
        .as_array()
        .context("cargo metadata packages must be an array")?;
    let nodes = metadata["resolve"]["nodes"]
        .as_array()
        .context("cargo metadata resolve nodes must be an array")?;

    let packages_by_id: BTreeMap<&str, &Value> = packages
        .iter()
        .filter_map(|package| Some((package["id"].as_str()?, package)))
        .collect();
    let dependencies_by_id: BTreeMap<&str, Vec<&str>> = nodes
        .iter()
        .filter_map(|node| {
            let id = node["id"].as_str()?;
            let dependencies = node["dependencies"]
                .as_array()?
                .iter()
                .filter_map(Value::as_str)
                .collect();
            Some((id, dependencies))
        })
        .collect();

    let roots = [
        "continuity-text",
        "continuity-buffer",
        "continuity-engine",
        "continuity-wasm",
        "continuity-engine-c",
        "continuity-python",
    ];
    let mut queue: VecDeque<&str> = packages
        .iter()
        .filter(|package| {
            package["name"]
                .as_str()
                .is_some_and(|name| roots.contains(&name))
        })
        .filter_map(|package| package["id"].as_str())
        .collect();
    let mut reached = BTreeSet::new();
    while let Some(id) = queue.pop_front() {
        if !reached.insert(id) {
            continue;
        }
        if let Some(dependencies) = dependencies_by_id.get(id) {
            queue.extend(dependencies.iter().copied());
        }
    }

    let mut components: Vec<Value> = reached
        .into_iter()
        .filter_map(|id| packages_by_id.get(id).copied())
        .map(cargo_component)
        .collect();
    components.push(json!({
        "type": "library",
        "name": config.npm.package,
        "version": config.sdk.version,
        "purl": format!(
            "pkg:npm/%40continuity-editor/editor@{}",
            config.sdk.version
        ),
        "properties": [{ "name": "continuity:surface", "value": "web-component" }]
    }));
    components.push(json!({
        "type": "library",
        "name": config.python.distribution,
        "version": config.sdk.version,
        "purl": format!("pkg:pypi/continuity-editor@{}", config.sdk.version),
        "properties": [{ "name": "continuity:surface", "value": "python-binding" }]
    }));
    components.sort_by(|left, right| {
        left["purl"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["purl"].as_str().unwrap_or_default())
    });

    let document = json!({
        "bomFormat": "CycloneDX",
        "serialNumber": compute_serial_number(&config.sdk.repository, &config.sdk.version),
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "component": {
                "type": "application",
                "name": "continuity-sdk",
                "version": config.sdk.version
            },
            "properties": [
                { "name": "continuity:channel", "value": config.sdk.channel },
                { "name": "continuity:repository", "value": config.sdk.repository }
            ]
        },
        "components": components
    });
    fs::write(output, serde_json::to_vec_pretty(&document)?)
        .with_context(|| format!("write SDK SBOM {}", output.display()))
}

fn compute_serial_number(repository: &str, version: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(repository.as_bytes());
    digest.update([0]);
    digest.update(version.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.finalize()[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "urn:uuid:{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

fn cargo_metadata(root: &Path) -> Result<Value> {
    let output = Command::new(env!("CARGO"))
        .current_dir(root)
        .args(["metadata", "--format-version", "1", "--locked"])
        .output()
        .context("run cargo metadata for SDK SBOM")?;
    if !output.status.success() {
        bail!(
            "cargo metadata for SDK SBOM failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    serde_json::from_slice(&output.stdout).context("parse cargo metadata for SDK SBOM")
}

fn cargo_component(package: &Value) -> Value {
    let name = package["name"].as_str().unwrap_or("unknown");
    let version = package["version"].as_str().unwrap_or("unknown");
    let mut component = json!({
        "type": "library",
        "name": name,
        "version": version,
        "purl": format!("pkg:cargo/{name}@{version}")
    });
    if let Some(license) = package["license"].as_str() {
        component["licenses"] = json!([{ "expression": license }]);
    }
    component
}

pub(crate) fn output_path(directory: &Path, version: &str) -> PathBuf {
    directory.join(format!("continuity-sdk-{version}.cdx.json"))
}

#[cfg(test)]
mod tests {
    use super::compute_serial_number;

    #[test]
    fn serial_number_is_stable_rfc_9562_uuid_v8() {
        let first = compute_serial_number("https://github.com/jatoran/continuity", "0.2.34");
        let second = compute_serial_number("https://github.com/jatoran/continuity", "0.2.34");
        let uuid = first
            .strip_prefix("urn:uuid:")
            .expect("invariant: CycloneDX serial number has UUID URN prefix");

        assert_eq!(first, second);
        assert_eq!(uuid.len(), 36);
        assert_eq!(uuid.as_bytes()[14], b'8');
        assert!(matches!(uuid.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
        assert_ne!(
            first,
            compute_serial_number("https://github.com/jatoran/continuity", "0.2.35")
        );
    }
}
