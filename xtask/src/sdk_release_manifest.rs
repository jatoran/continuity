//! Canonical SDK release coordinates and wrapper-version verification.

use std::fs;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;

const RELEASE_MANIFEST: &str = "sdk/release.toml";

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct ReleaseConfig {
    pub(crate) sdk: SdkConfig,
    pub(crate) cargo: CargoConfig,
    pub(crate) npm: NpmConfig,
    pub(crate) python: PythonConfig,
    pub(crate) c_abi: CAbiConfig,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct SdkConfig {
    pub(crate) version: String,
    pub(crate) channel: String,
    pub(crate) msrv: String,
    pub(crate) tag_prefix: String,
    pub(crate) repository: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CargoConfig {
    pub(crate) packages: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct NpmConfig {
    pub(crate) package: String,
    pub(crate) preview_tag: String,
    pub(crate) stable_tag: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct PythonConfig {
    pub(crate) distribution: String,
    pub(crate) minimum_version: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CAbiConfig {
    pub(crate) major: u32,
    pub(crate) minor: u32,
    pub(crate) target: String,
}

pub(crate) fn load(root: &Path) -> Result<ReleaseConfig> {
    let path = root.join(RELEASE_MANIFEST);
    let contents = fs::read_to_string(&path)
        .with_context(|| format!("read canonical SDK release manifest {}", path.display()))?;
    toml::from_str(&contents).context("parse canonical SDK release manifest")
}

pub(crate) fn canonical_version(root: &Path) -> Result<String> {
    Ok(load(root)?.sdk.version)
}

pub(crate) fn verify(root: &Path) -> Result<ReleaseConfig> {
    let config = load(root)?;
    verify_release_config(&config)?;
    verify_workspace(root, &config)?;
    verify_rust_wrappers(root, &config)?;
    verify_npm_wrapper(root, &config)?;
    verify_python_wrapper(root, &config)?;
    verify_c_header(root, &config)?;
    Ok(config)
}

fn verify_release_config(config: &ReleaseConfig) -> Result<()> {
    if config.cargo.packages != ["continuity-text", "continuity-buffer", "continuity-engine"] {
        bail!("Cargo packages must remain in dependency publication order");
    }
    if config.sdk.channel != "preview" && config.sdk.channel != "stable" {
        bail!("SDK channel must be `preview` or `stable`");
    }
    if config.sdk.tag_prefix != "sdk-v" {
        bail!("SDK tag prefix must remain separate from desktop `v*` tags");
    }
    if config.npm.preview_tag == config.npm.stable_tag {
        bail!("npm preview and stable distribution tags must differ");
    }
    if config.c_abi.major == 0 {
        bail!("the published C ABI must have a non-zero major version");
    }
    Ok(())
}

fn verify_workspace(root: &Path, config: &ReleaseConfig) -> Result<()> {
    let workspace = parse_toml(&root.join("Cargo.toml"))?;
    let package = table_path(&workspace, &["workspace", "package"])?;
    require_string(package, "rust-version", &config.sdk.msrv, "workspace MSRV")?;
    require_string(
        package,
        "repository",
        &config.sdk.repository,
        "workspace repository",
    )?;
    let dependencies = table_path(&workspace, &["workspace", "dependencies"])?;
    for name in &config.cargo.packages {
        let dependency = dependencies
            .get(name)
            .and_then(toml::Value::as_table)
            .ok_or_else(|| anyhow!("workspace dependency `{name}` must be a table"))?;
        require_string(
            dependency,
            "version",
            &config.sdk.version,
            &format!("workspace dependency `{name}`"),
        )?;
    }
    Ok(())
}

fn verify_rust_wrappers(root: &Path, config: &ReleaseConfig) -> Result<()> {
    for manifest in [
        "crates/text/Cargo.toml",
        "crates/buffer/Cargo.toml",
        "crates/engine/Cargo.toml",
        "crates/wasm/Cargo.toml",
        "crates/c_api/Cargo.toml",
        "bindings/python/Cargo.toml",
    ] {
        let value = parse_toml(&root.join(manifest))?;
        let package = table_path(&value, &["package"])?;
        require_string(package, "version", &config.sdk.version, manifest)?;
    }
    Ok(())
}

fn verify_npm_wrapper(root: &Path, config: &ReleaseConfig) -> Result<()> {
    let path = root.join("packages/editor/package.json");
    let value: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path)?)?;
    require_json_string(&value, "name", &config.npm.package, "npm package name")?;
    require_json_string(
        &value,
        "version",
        &config.sdk.version,
        "npm package version",
    )?;
    let expected_repository = format!("git+{}.git", config.sdk.repository);
    let repository = value
        .pointer("/repository/url")
        .and_then(serde_json::Value::as_str);
    if repository != Some(expected_repository.as_str()) {
        bail!("npm repository must be `{expected_repository}` for trusted publishing");
    }
    Ok(())
}

fn verify_python_wrapper(root: &Path, config: &ReleaseConfig) -> Result<()> {
    let value = parse_toml(&root.join("bindings/python/pyproject.toml"))?;
    let project = table_path(&value, &["project"])?;
    require_string(
        project,
        "name",
        &config.python.distribution,
        "Python distribution",
    )?;
    require_string(
        project,
        "version",
        &config.sdk.version,
        "Python distribution",
    )?;
    let minimum = format!(">={}", config.python.minimum_version);
    require_string(
        project,
        "requires-python",
        &minimum,
        "Python minimum version",
    )
}

fn verify_c_header(root: &Path, config: &ReleaseConfig) -> Result<()> {
    let header = fs::read_to_string(root.join("crates/c_api/include/continuity_engine.h"))?;
    for required in [
        format!("#define CONTINUITY_ENGINE_ABI_MAJOR {}", config.c_abi.major),
        format!("#define CONTINUITY_ENGINE_ABI_MINOR {}", config.c_abi.minor),
        format!(
            "#define CONTINUITY_ENGINE_SDK_VERSION \"{}\"",
            config.sdk.version
        ),
    ] {
        if !header.contains(&required) {
            bail!("C header is missing canonical release contract `{required}`");
        }
    }
    Ok(())
}

fn parse_toml(path: &Path) -> Result<toml::Value> {
    let contents = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&contents).with_context(|| format!("parse {}", path.display()))
}

fn table_path<'a>(value: &'a toml::Value, path: &[&str]) -> Result<&'a toml::Table> {
    let mut current = value;
    for part in path {
        current = current
            .get(part)
            .ok_or_else(|| anyhow!("missing TOML path `{}`", path.join(".")))?;
    }
    current
        .as_table()
        .ok_or_else(|| anyhow!("TOML path `{}` must be a table", path.join(".")))
}

fn require_string(table: &toml::Table, key: &str, expected: &str, context: &str) -> Result<()> {
    let actual = table.get(key).and_then(toml::Value::as_str);
    if actual != Some(expected) {
        bail!("{context} `{key}` must be `{expected}`, found {actual:?}");
    }
    Ok(())
}

fn require_json_string(
    value: &serde_json::Value,
    key: &str,
    expected: &str,
    context: &str,
) -> Result<()> {
    let actual = value.get(key).and_then(serde_json::Value::as_str);
    if actual != Some(expected) {
        bail!("{context} must be `{expected}`, found {actual:?}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{verify_release_config, CAbiConfig, CargoConfig, NpmConfig, PythonConfig};
    use super::{ReleaseConfig, SdkConfig};

    fn config() -> ReleaseConfig {
        ReleaseConfig {
            sdk: SdkConfig {
                version: "0.1.0".into(),
                channel: "preview".into(),
                msrv: "1.89".into(),
                tag_prefix: "sdk-v".into(),
                repository: "https://github.com/jatoran/continuity".into(),
            },
            cargo: CargoConfig {
                packages: vec![
                    "continuity-text".into(),
                    "continuity-buffer".into(),
                    "continuity-engine".into(),
                ],
            },
            npm: NpmConfig {
                package: "@continuity-editor/editor".into(),
                preview_tag: "next".into(),
                stable_tag: "latest".into(),
            },
            python: PythonConfig {
                distribution: "continuity-editor".into(),
                minimum_version: "3.10".into(),
            },
            c_abi: CAbiConfig {
                major: 1,
                minor: 0,
                target: "x86_64-pc-windows-msvc".into(),
            },
        }
    }

    #[test]
    fn canonical_release_configuration_is_valid() {
        verify_release_config(&config()).expect("invariant: valid release config");
    }

    #[test]
    fn cargo_publication_order_is_contractual() {
        let mut value = config();
        value.cargo.packages.reverse();
        assert!(verify_release_config(&value).is_err());
    }
}
