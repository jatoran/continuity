//! Theme-key generated docs.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::docs_gen::rust_source::brace_delta;
use crate::docs_gen::{escape_md_cell, new_doc, normalize_path};

pub(crate) fn write_theme_keys(workspace: &Path) -> Result<String> {
    let keys = required_keys(workspace)?;
    let assets = bundled_theme_assets(workspace)?;
    let coverage = theme_coverage(&assets)?;
    let accessors = accessor_map(
        workspace,
        keys.iter().map(|key| key.name.as_str()).collect(),
    )?;

    let mut out = new_doc("Theme Keys");
    out.push_str("Generated from `crates/theme/src/keys.rs`, typed theme accessors, and bundled theme TOML assets.\n\n");
    out.push_str("| Key | Group | Source | Bundled coverage | Accessor(s) |\n");
    out.push_str("|---|---|---|---|---|\n");
    for key in keys {
        let missing = coverage
            .iter()
            .filter_map(|(asset, keys)| (!keys.contains(&key.name)).then_some(asset.as_str()))
            .collect::<Vec<_>>();
        let bundled = if missing.is_empty() {
            format!("{}/{}", coverage.len(), coverage.len())
        } else {
            format!("missing {}", missing.join(", "))
        };
        let accessor = accessors
            .get(&key.name)
            .map(|items| items.join("<br>"))
            .unwrap_or_default();
        out.push_str(&format!(
            "| `{}` | `{}` | `crates/theme/src/keys.rs`:{} | {} | {} |\n",
            key.name,
            key.group(),
            key.line,
            escape_md_cell(&bundled),
            accessor
        ));
    }
    Ok(out)
}

#[derive(Clone, Debug)]
struct ThemeKey {
    name: String,
    line: usize,
}

impl ThemeKey {
    fn group(&self) -> &str {
        self.name
            .split_once('.')
            .map_or(&self.name, |(group, _)| group)
    }
}

fn required_keys(workspace: &Path) -> Result<Vec<ThemeKey>> {
    let path = workspace.join("crates/theme/src/keys.rs");
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let mut keys = Vec::new();
    let mut in_required = false;
    for (idx, line) in text.lines().enumerate() {
        if line.contains("REQUIRED_KEYS") {
            in_required = true;
            continue;
        }
        if in_required && line.trim_start().starts_with("];") {
            break;
        }
        if !in_required {
            continue;
        }
        for key in quoted_strings(line) {
            if key.contains('.') {
                keys.push(ThemeKey {
                    name: key,
                    line: idx + 1,
                });
            }
        }
    }
    Ok(keys)
}

fn bundled_theme_assets(workspace: &Path) -> Result<Vec<PathBuf>> {
    let dir = workspace.join("crates/theme/assets");
    let mut files = fs::read_dir(&dir)
        .with_context(|| format!("listing {}", dir.display()))?
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("reading {}", dir.display()))?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

fn theme_coverage(assets: &[PathBuf]) -> Result<BTreeMap<String, BTreeSet<String>>> {
    let mut out = BTreeMap::new();
    for asset in assets {
        let text =
            fs::read_to_string(asset).with_context(|| format!("reading {}", asset.display()))?;
        let value: toml::Value =
            toml::from_str(&text).with_context(|| format!("parsing {}", asset.display()))?;
        let keys = value
            .get("colors")
            .and_then(toml::Value::as_table)
            .map(|table| table.keys().cloned().collect::<BTreeSet<_>>())
            .unwrap_or_default();
        out.insert(asset.file_stem_name(), keys);
    }
    Ok(out)
}

fn accessor_map(
    workspace: &Path,
    required: BTreeSet<&str>,
) -> Result<BTreeMap<String, Vec<String>>> {
    let files = [
        "crates/theme/src/theme.rs",
        "crates/theme/src/theme/markdown_accessors.rs",
    ];
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for relative in files {
        let path = workspace.join(relative);
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let mut current_fn: Option<String> = None;
        let mut fn_depth = 0isize;
        for line in text.lines() {
            let trimmed = line.trim_start();
            if current_fn.is_none() {
                if let Some(rest) = trimmed.strip_prefix("pub fn ") {
                    current_fn = Some(
                        rest.chars()
                            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                            .collect(),
                    );
                    fn_depth = brace_delta(line).max(0);
                }
            } else {
                fn_depth += brace_delta(line);
            }
            if let Some(fn_name) = current_fn.as_ref() {
                for key in quoted_strings(line) {
                    if required.contains(key.as_str()) {
                        out.entry(key).or_default().push(format!("`{fn_name}`"));
                    }
                }
                if fn_depth <= 0 {
                    current_fn = None;
                }
            }
        }
    }
    for values in out.values_mut() {
        values.sort();
        values.dedup();
    }
    Ok(out)
}

fn quoted_strings(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find('"') {
        let after_start = &rest[start + 1..];
        let Some(end) = after_start.find('"') else {
            break;
        };
        out.push(after_start[..end].to_string());
        rest = &after_start[end + 1..];
    }
    out
}

trait FileStemName {
    fn file_stem_name(&self) -> String;
}

impl FileStemName for PathBuf {
    fn file_stem_name(&self) -> String {
        self.file_stem()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| normalize_path(self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_strings_extracts_theme_keys() {
        assert_eq!(
            quoted_strings("\"editor.background\" = \"#000000\""),
            vec!["editor.background", "#000000"]
        );
    }
}
