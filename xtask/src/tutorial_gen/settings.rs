//! Settings appendix generator + the rustdoc parser that backs it.
//!
//! Sibling of `xtask/src/tutorial_gen.rs`. Holds the
//! `write_settings` entry point plus the line-oriented `pub struct`
//! parser (`parse_pub_structs`) so the parent generator file stays
//! under the 600-line cap.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

/// One `[section]` header in `settings.toml`, paired with the `*Config`
/// struct that backs it. Discovered by parsing the `Settings` outer
/// struct in `crates/config/src/settings.rs`.
#[derive(Debug)]
pub(crate) struct SettingsSection {
    pub toml_name: String,
    pub struct_name: String,
}

/// One settings field: TOML key, the Rust type literal as written in
/// source, and the rustdoc `///` comment block (concatenated lines).
#[derive(Debug)]
pub(crate) struct SettingsField {
    pub name: String,
    pub ty: String,
    pub doc: String,
}

/// Path of every Rust source file in `crates/config/src/` that may
/// host a `*Config` struct backing a `[section]` of `settings.toml`.
const CONFIG_SRC_FILES: &[&str] = &[
    "crates/config/src/settings.rs",
    "crates/config/src/focus.rs",
    "crates/config/src/workers.rs",
];

/// Emit a Settings appendix by parsing rustdoc on the `Settings`
/// struct fields (to get section names) and every `*Config` struct
/// in `crates/config/src/`. Output: one H3 per section, then a
/// markdown table of field name + type + description.
///
/// Best-effort parser — recognises `pub struct Foo {`, `pub <name>:
/// <type>,`, and `/// <doc>` comments. Lines that don't match the
/// expected shape are ignored. The CI drift check makes any parser
/// failure visible immediately.
pub(crate) fn write_settings(out: &mut String, workspace: &Path) -> Result<()> {
    let mut all_structs: BTreeMap<String, Vec<SettingsField>> = BTreeMap::new();
    for relative in CONFIG_SRC_FILES {
        let path = workspace.join(relative);
        let text = fs::read_to_string(&path)
            .with_context(|| format!("reading settings source {}", path.display()))?;
        let structs = parse_pub_structs(&text);
        for (name, fields) in structs {
            all_structs.insert(name, fields);
        }
    }
    let sections = match all_structs.get("Settings") {
        Some(fields) => fields
            .iter()
            .map(|f| SettingsSection {
                toml_name: f.name.clone(),
                struct_name: f.ty.clone(),
            })
            .collect::<Vec<_>>(),
        None => Vec::new(),
    };

    out.push_str("## Settings\n");
    out.push('\n');
    out.push_str(
        "_Auto-extracted from rustdoc `///` comments on the `Settings` struct and the \
         per-section `*Config` structs in `crates/config/src/`. One section per `[toml.section]`._\n",
    );
    out.push('\n');
    for section in sections {
        let Some(fields) = all_structs.get(&section.struct_name) else {
            continue;
        };
        out.push_str("### `[");
        out.push_str(&section.toml_name);
        out.push_str("]`\n");
        out.push('\n');
        out.push_str("| Field | Type | Description |\n");
        out.push_str("|---|---|---|\n");
        for field in fields {
            out.push_str("| `");
            out.push_str(&field.name);
            out.push_str("` | `");
            out.push_str(&field.ty);
            out.push_str("` | ");
            out.push_str(&crate::tutorial_gen::escape_md_table_cell(&field.doc));
            out.push_str(" |\n");
        }
        out.push('\n');
    }
    Ok(())
}

/// Tiny state-machine parser: walks Rust source line-by-line, tracks
/// the current `pub struct Name {`, accumulates `///` doc comments
/// preceding each `pub <field>: <type>,` line. Returns one entry per
/// struct found.
pub(crate) fn parse_pub_structs(source: &str) -> BTreeMap<String, Vec<SettingsField>> {
    let mut out: BTreeMap<String, Vec<SettingsField>> = BTreeMap::new();
    let mut current_struct: Option<String> = None;
    let mut current_fields: Vec<SettingsField> = Vec::new();
    let mut pending_doc: Vec<String> = Vec::new();
    let mut brace_depth: i32 = 0;

    for raw_line in source.lines() {
        let trimmed = raw_line.trim();
        if let Some(doc_text) = trimmed.strip_prefix("///") {
            // Accumulate doc only when *inside* a struct; outer
            // `///` comments above the struct itself are ignored.
            if current_struct.is_some() {
                pending_doc.push(doc_text.trim().to_string());
            }
            continue;
        }
        // Track struct open / close. Only top-level `pub struct Foo {`
        // declarations matter — nested structs are rare in this crate.
        if current_struct.is_none() {
            if let Some(name) = parse_struct_open(trimmed) {
                current_struct = Some(name);
                current_fields.clear();
                pending_doc.clear();
                brace_depth = 1;
                continue;
            }
        } else {
            brace_depth += count_chars(trimmed, '{');
            brace_depth -= count_chars(trimmed, '}');
            if brace_depth <= 0 {
                if let Some(name) = current_struct.take() {
                    out.insert(name, std::mem::take(&mut current_fields));
                }
                pending_doc.clear();
                continue;
            }
            if let Some(field) = parse_pub_field(trimmed) {
                let doc = pending_doc.join(" ").trim().to_string();
                current_fields.push(SettingsField {
                    name: field.0,
                    ty: field.1,
                    doc,
                });
                pending_doc.clear();
            } else if !trimmed.is_empty() && !trimmed.starts_with("//") {
                // Reset doc accumulator on non-field non-blank line so a
                // doc above a non-field item (e.g. a method) doesn't
                // attach to the next field.
                pending_doc.clear();
            }
        }
    }
    out
}

fn parse_struct_open(line: &str) -> Option<String> {
    // Match `pub struct Foo {` or `pub(crate) struct Foo {` shapes.
    let rest = line
        .strip_prefix("pub struct ")
        .or_else(|| line.strip_prefix("pub(crate) struct "))?;
    let name_end = rest
        .find(|c: char| !c.is_alphanumeric() && c != '_')
        .unwrap_or(rest.len());
    let name = &rest[..name_end];
    if name.is_empty() || !line.contains('{') {
        return None;
    }
    Some(name.to_string())
}

fn parse_pub_field(line: &str) -> Option<(String, String)> {
    // `pub <name>: <type>,` with optional trailing comma.
    let rest = line.strip_prefix("pub ")?;
    let colon = rest.find(':')?;
    let name = rest[..colon].trim();
    if name.is_empty() || !is_identifier(name) {
        return None;
    }
    let ty_raw = rest[colon + 1..].trim();
    let ty = ty_raw.trim_end_matches(',').trim().to_string();
    if ty.is_empty() || ty.starts_with('{') {
        return None;
    }
    Some((name.to_string(), ty))
}

fn is_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !s.chars().next().is_some_and(|c| c.is_numeric())
}

fn count_chars(s: &str, target: char) -> i32 {
    i32::try_from(s.chars().filter(|&c| c == target).count()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pub_structs_extracts_fields_and_docs() {
        let source = r#"
/// outer doc — ignored
pub struct Demo {
    /// First field.
    pub one: bool,
    /// Second field
    /// across two lines.
    pub two: String,
}
"#;
        let parsed = parse_pub_structs(source);
        let fields = parsed.get("Demo").expect("Demo struct parsed");
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "one");
        assert_eq!(fields[0].ty, "bool");
        assert_eq!(fields[0].doc, "First field.");
        assert_eq!(fields[1].name, "two");
        assert_eq!(fields[1].ty, "String");
        assert!(fields[1].doc.contains("Second field"));
        assert!(fields[1].doc.contains("across two lines."));
    }

    #[test]
    fn parse_pub_structs_handles_multiple_structs() {
        let source = r#"
pub struct A {
    /// a
    pub a_field: i32,
}

pub struct B {
    /// b
    pub b_field: i32,
}
"#;
        let parsed = parse_pub_structs(source);
        assert!(parsed.contains_key("A"));
        assert!(parsed.contains_key("B"));
    }

    #[test]
    fn parse_pub_structs_skips_non_field_pub_items() {
        let source = r#"
pub struct C {
    /// only field
    pub field_one: i32,
}

impl C {
    /// method doc
    pub fn method(&self) -> i32 { 0 }
}
"#;
        let parsed = parse_pub_structs(source);
        let fields = parsed.get("C").expect("C parsed");
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].name, "field_one");
    }
}
