//! CycloneDX inventory for the Rust, npm, and Python SDK release closure.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

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
