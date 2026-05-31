//! Lightweight Rust-source scanning helpers for generated docs.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::docs_gen::{line_count, normalize_path, relative_path};

#[derive(Clone, Debug)]
pub(crate) struct WorkspaceCrate {
    pub(crate) member: String,
    pub(crate) package_name: String,
    pub(crate) path: String,
}

#[derive(Clone, Debug)]
pub(crate) struct RustSource {
    pub(crate) relative: String,
    pub(crate) module_path: String,
    pub(crate) text: String,
    pub(crate) lines: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct PublicItem {
    pub(crate) kind: String,
    pub(crate) name: String,
    pub(crate) signature: String,
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) doc: String,
}

#[derive(Clone, Debug)]
pub(crate) struct EnumVariant {
    pub(crate) name: String,
    pub(crate) payload: String,
    pub(crate) line: usize,
    pub(crate) doc: String,
}

pub(crate) fn workspace_crates(workspace: &Path) -> Result<Vec<WorkspaceCrate>> {
    let manifest_path = workspace.join("Cargo.toml");
    let text = fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let root: toml::Value =
        toml::from_str(&text).with_context(|| format!("parsing {}", manifest_path.display()))?;
    let members = root
        .get("workspace")
        .and_then(|value| value.get("members"))
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str);
    let mut crates = Vec::new();
    for member in members {
        let member_path = workspace.join(member);
        let manifest = read_toml(&member_path.join("Cargo.toml"))?;
        let package_name = manifest
            .get("package")
            .and_then(|value| value.get("name"))
            .and_then(toml::Value::as_str)
            .unwrap_or(member)
            .to_string();
        crates.push(WorkspaceCrate {
            member: member_name(member),
            package_name,
            path: member.to_string(),
        });
    }
    Ok(crates)
}

pub(crate) fn crate_rust_sources(
    workspace: &Path,
    krate: &WorkspaceCrate,
) -> Result<Vec<RustSource>> {
    let src_dir = workspace.join(&krate.path).join("src");
    let mut files = Vec::new();
    if src_dir.exists() {
        collect_rust_sources(workspace, &krate.path, &src_dir, &mut files)?;
    }
    files.sort_by(|a, b| a.relative.cmp(&b.relative));
    Ok(files)
}

pub(crate) fn first_doc_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find_map(|line| {
            line.strip_prefix("//!")
                .or_else(|| line.strip_prefix("///"))
                .map(clean_doc_text)
                .filter(|line| !line.is_empty())
        })
        .unwrap_or_default()
}

pub(crate) fn parse_public_items(source: &RustSource) -> Vec<PublicItem> {
    let mut items = Vec::new();
    let mut docs = Vec::new();
    let mut depth = 0isize;
    for (idx, raw) in source.text.lines().enumerate() {
        let trimmed = raw.trim();
        if depth == 0 {
            if let Some(doc) = trimmed.strip_prefix("///") {
                docs.push(clean_doc_text(doc));
                continue;
            }
            if trimmed.starts_with("#[") {
                continue;
            }
            if let Some((kind, name)) = parse_public_item_name(trimmed) {
                items.push(PublicItem {
                    kind,
                    name,
                    signature: trimmed.trim_end_matches('{').trim().to_string(),
                    path: source.relative.clone(),
                    line: idx + 1,
                    doc: docs
                        .iter()
                        .find(|line| !line.is_empty())
                        .cloned()
                        .unwrap_or_default(),
                });
                docs.clear();
            } else if !trimmed.is_empty() {
                docs.clear();
            }
        }
        depth += brace_delta(raw);
        depth = depth.max(0);
    }
    items
}

pub(crate) fn parse_enum_variants(text: &str, enum_name: &str) -> Vec<EnumVariant> {
    let lines = text.lines().collect::<Vec<_>>();
    let Some(start) = lines.iter().position(|line| {
        let trimmed = line.trim();
        trimmed.starts_with("pub enum ")
            && trimmed
                .strip_prefix("pub enum ")
                .is_some_and(|rest| rest.starts_with(enum_name))
    }) else {
        return Vec::new();
    };

    let mut variants = Vec::new();
    let mut docs = Vec::new();
    let mut depth = brace_delta(lines[start]).max(0);
    let mut current: Option<VariantBuilder> = None;

    for (idx, raw) in lines.iter().enumerate().skip(start + 1) {
        let trimmed = raw.trim();
        if let Some(builder) = current.as_mut() {
            if trimmed.starts_with("},") || trimmed == "}" {
                let builder = current.take().expect("builder exists");
                variants.push(builder.finish());
            } else if let Some(field) = parse_field_name(trimmed) {
                builder.fields.push(field);
            }
        } else if depth == 1 {
            if let Some(doc) = trimmed.strip_prefix("///") {
                docs.push(clean_doc_text(doc));
                depth += brace_delta(raw);
                continue;
            }
            if trimmed.starts_with("#[") || trimmed.is_empty() {
                depth += brace_delta(raw);
                continue;
            }
            if let Some(started) = parse_variant_start(trimmed, idx + 1, &docs) {
                docs.clear();
                if started.is_struct {
                    current = Some(started);
                } else {
                    variants.push(started.finish());
                }
            } else {
                docs.clear();
            }
        }
        depth += brace_delta(raw);
        if depth <= 0 {
            break;
        }
    }
    variants
}

pub(crate) fn clean_doc_text(s: &str) -> String {
    s.trim().trim_start_matches('/').trim().to_string()
}

pub(crate) fn brace_delta(line: &str) -> isize {
    let mut delta = 0;
    let mut chars = line.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if !in_string && ch == '/' && chars.peek() == Some(&'/') {
            break;
        }
        if ch == '"' && !escaped {
            in_string = !in_string;
        }
        escaped = in_string && ch == '\\' && !escaped;
        if in_string {
            continue;
        }
        if ch == '{' {
            delta += 1;
        } else if ch == '}' {
            delta -= 1;
        }
    }
    delta
}

fn collect_rust_sources(
    workspace: &Path,
    crate_path: &str,
    dir: &Path,
    out: &mut Vec<RustSource>,
) -> Result<()> {
    let mut entries = fs::read_dir(dir)
        .with_context(|| format!("listing {}", dir.display()))?
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("reading {}", dir.display()))?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_sources(workspace, crate_path, &path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let text =
                fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            let relative = relative_path(workspace, &path)?;
            out.push(RustSource {
                module_path: module_path(crate_path, &relative),
                lines: line_count(&text),
                relative,
                text,
            });
        }
    }
    Ok(())
}

fn module_path(crate_path: &str, relative: &str) -> String {
    let prefix = format!("{crate_path}/src/");
    let Some(rest) = relative.strip_prefix(&prefix) else {
        return normalize_path(Path::new(relative));
    };
    if rest == "lib.rs" || rest == "main.rs" {
        return "crate".into();
    }
    rest.trim_end_matches(".rs").replace('/', "::")
}

fn read_toml(path: &Path) -> Result<toml::Value> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

fn member_name(member: &str) -> String {
    member
        .rsplit_once('/')
        .map_or(member, |(_, name)| name)
        .to_string()
}

fn parse_public_item_name(line: &str) -> Option<(String, String)> {
    if line.starts_with("pub(crate)")
        || line.starts_with("pub(super)")
        || line.starts_with("pub(in ")
    {
        return None;
    }
    let rest = line.strip_prefix("pub ")?;
    for (keyword, kind) in [
        ("use ", "use"),
        ("mod ", "mod"),
        ("struct ", "struct"),
        ("enum ", "enum"),
        ("trait ", "trait"),
        ("type ", "type"),
        ("fn ", "fn"),
        ("const ", "const"),
        ("static ", "static"),
    ] {
        if let Some(after) = rest.strip_prefix(keyword) {
            let name = if kind == "use" {
                after.trim_end_matches(';').trim().to_string()
            } else {
                take_identifier(after)
            };
            if !name.is_empty() {
                return Some((kind.into(), name));
            }
        }
    }
    None
}

fn parse_variant_start(line: &str, line_number: usize, docs: &[String]) -> Option<VariantBuilder> {
    let name = take_identifier(line);
    if name.is_empty() || !name.chars().next().is_some_and(char::is_uppercase) {
        return None;
    }
    let rest = line[name.len()..].trim_start();
    let mut builder = VariantBuilder {
        name,
        payload: String::new(),
        fields: Vec::new(),
        line: line_number,
        doc: docs
            .iter()
            .find(|line| !line.is_empty())
            .cloned()
            .unwrap_or_default(),
        is_struct: false,
    };
    if let Some(tuple) = rest.strip_prefix('(') {
        builder.payload = tuple
            .split_once(')')
            .map_or(tuple, |(payload, _)| payload)
            .trim()
            .to_string();
    } else if rest.starts_with('{') {
        builder.is_struct = true;
    }
    Some(builder)
}

fn parse_field_name(line: &str) -> Option<String> {
    if line.starts_with("///") || line.starts_with("#[") {
        return None;
    }
    let (name, _) = line.split_once(':')?;
    let name = name.trim();
    if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        Some(name.to_string())
    } else {
        None
    }
}

fn take_identifier(s: &str) -> String {
    s.chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect()
}

struct VariantBuilder {
    name: String,
    payload: String,
    fields: Vec<String>,
    line: usize,
    doc: String,
    is_struct: bool,
}

impl VariantBuilder {
    fn finish(mut self) -> EnumVariant {
        if self.payload.is_empty() && !self.fields.is_empty() {
            self.payload = format!("fields: {}", self.fields.join(", "));
        }
        EnumVariant {
            name: self.name,
            payload: self.payload,
            line: self.line,
            doc: self.doc,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_public_top_level_items_only() {
        let source = RustSource {
            relative: "crates/demo/src/lib.rs".into(),
            module_path: "crate".into(),
            text: r#"
/// Public type.
pub struct Demo;

impl Demo {
    pub fn method(&self) {}
}

pub(crate) struct Hidden;
pub use crate::demo::Demo as Renamed;
"#
            .into(),
            lines: 10,
        };
        let items = parse_public_items(&source);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "Demo");
        assert_eq!(items[0].doc, "Public type.");
        assert_eq!(items[1].kind, "use");
    }

    #[test]
    fn parses_enum_struct_and_tuple_variants() {
        let variants = parse_enum_variants(
            r#"
pub enum Demo {
    /// Unit docs.
    Unit,
    /// Tuple docs.
    Tuple(String),
    /// Struct docs.
    Struct {
        /// A field.
        value: u32,
    },
}
"#,
            "Demo",
        );
        assert_eq!(variants.len(), 3);
        assert_eq!(variants[1].payload, "String");
        assert_eq!(variants[2].payload, "fields: value");
    }
}
