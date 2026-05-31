//! Settings generated docs.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::docs_gen::{escape_md_cell, new_doc};
use crate::tutorial_gen::settings::{parse_pub_structs, SettingsField, SettingsSection};

#[derive(Clone, Debug)]
pub(crate) struct SettingDoc {
    pub(crate) section: String,
    pub(crate) key: String,
    pub(crate) rust_field: String,
    pub(crate) ty: String,
    pub(crate) default: String,
    pub(crate) validation_hint: String,
    pub(crate) description: String,
    pub(crate) source_path: String,
}

const CONFIG_SRC_FILES: &[&str] = &[
    "crates/config/src/settings.rs",
    "crates/config/src/focus.rs",
    "crates/config/src/workers.rs",
];

pub(crate) fn write_settings(workspace: &Path) -> Result<String> {
    let settings = collect_settings(workspace)?;
    let mut by_section: BTreeMap<String, Vec<SettingDoc>> = BTreeMap::new();
    for setting in settings {
        by_section
            .entry(setting.section.clone())
            .or_default()
            .push(setting);
    }

    let mut out = new_doc("Settings");
    out.push_str(
        "Generated from rustdoc fields and `impl Default` blocks in `crates/config/src/`.\n",
    );
    out.push_str("Validation hints are best-effort extracts from field rustdoc.\n\n");
    for (section, settings) in by_section {
        out.push_str(&format!("## `[{section}]`\n\n"));
        out.push_str("| Key | Rust field | Type | Default | Validation hint | Description |\n");
        out.push_str("|---|---|---|---|---|---|\n");
        for setting in settings {
            out.push_str(&format!(
                "| `{}` | `{}` | `{}` | {} | {} | {} |\n",
                setting.key,
                setting.rust_field,
                escape_md_cell(&setting.ty),
                format_default(&setting.default),
                escape_md_cell(&setting.validation_hint),
                escape_md_cell(&setting.description)
            ));
        }
        out.push('\n');
    }
    Ok(out)
}

pub(crate) fn collect_settings(workspace: &Path) -> Result<Vec<SettingDoc>> {
    let mut structs: BTreeMap<String, Vec<SettingsField>> = BTreeMap::new();
    let mut defaults: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut struct_sources: BTreeMap<String, String> = BTreeMap::new();
    for relative in CONFIG_SRC_FILES {
        let path = workspace.join(relative);
        let text = fs::read_to_string(&path)
            .with_context(|| format!("reading settings source {}", path.display()))?;
        for (name, fields) in parse_pub_structs(&text) {
            struct_sources.insert(name.clone(), (*relative).to_string());
            structs.insert(name, fields);
        }
        for (name, fields) in parse_default_fields(&text) {
            defaults.insert(name, fields);
        }
    }

    let sections = structs
        .get("Settings")
        .map(|fields| {
            fields
                .iter()
                .map(|field| SettingsSection {
                    toml_name: field.name.clone(),
                    struct_name: field.ty.clone(),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let mut out = Vec::new();
    for section in sections {
        let Some(fields) = structs.get(&section.struct_name) else {
            continue;
        };
        let section_defaults = defaults.get(&section.struct_name);
        let source_path = struct_sources
            .get(&section.struct_name)
            .cloned()
            .unwrap_or_else(|| "crates/config/src/settings.rs".into());
        for field in fields {
            let default = section_defaults
                .and_then(|fields| fields.get(&field.name))
                .cloned()
                .unwrap_or_default();
            out.push(SettingDoc {
                section: section.toml_name.clone(),
                key: field.name.clone(),
                rust_field: field.name.clone(),
                ty: field.ty.clone(),
                default,
                validation_hint: validation_hint(&field.doc),
                description: field.doc.clone(),
                source_path: source_path.clone(),
            });
        }
    }
    Ok(out)
}

fn parse_default_fields(source: &str) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    let mut current_struct: Option<String> = None;
    let mut in_self = false;
    let mut fields = BTreeMap::new();

    for raw_line in source.lines() {
        let trimmed = raw_line.trim();
        if current_struct.is_none() {
            if let Some(rest) = trimmed.strip_prefix("impl Default for ") {
                let name = rest
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .trim_end_matches('{');
                if !name.is_empty() {
                    current_struct = Some(name.to_string());
                    fields.clear();
                }
            }
            continue;
        }

        if !in_self {
            if trimmed.starts_with("Self {") {
                in_self = true;
            }
            continue;
        }

        if trimmed.starts_with('}') {
            if let Some(name) = current_struct.take() {
                out.insert(name, std::mem::take(&mut fields));
            }
            in_self = false;
            continue;
        }

        if let Some((name, value)) = parse_default_line(trimmed) {
            fields.insert(name, value);
        }
    }
    out
}

fn parse_default_line(line: &str) -> Option<(String, String)> {
    let (name, raw_value) = line.split_once(':')?;
    let name = name.trim();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let value = raw_value.trim().trim_end_matches(',').trim();
    if value.is_empty() || value == "vec![" {
        return None;
    }
    Some((name.to_string(), value.to_string()))
}

fn format_default(default: &str) -> String {
    if default.is_empty() {
        return String::new();
    }
    format!("`{}`", escape_md_cell(default))
}

fn validation_hint(doc: &str) -> String {
    doc.split('.')
        .map(str::trim)
        .find(|sentence| {
            sentence.contains('|')
                || sentence.contains("0..")
                || sentence.contains("One of")
                || sentence.contains("Single character")
        })
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_line_default_fields() {
        let source = r#"
impl Default for Demo {
    fn default() -> Self {
        Self {
            enabled: true,
            name: "demo".into(),
        }
    }
}
"#;
        let parsed = parse_default_fields(source);
        assert_eq!(parsed["Demo"]["enabled"], "true");
        assert_eq!(parsed["Demo"]["name"], "\"demo\".into()");
    }

    #[test]
    fn validation_hint_extracts_choice_sentence() {
        let doc = "\"bar\" | \"block\" | \"underline\". Other text.";
        assert_eq!(validation_hint(doc), "\"bar\" | \"block\" | \"underline\"");
    }
}
