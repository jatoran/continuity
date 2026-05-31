//! Workspace inventory generation for generated docs.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::docs_gen::{escape_md_cell, line_count, new_doc, normalize_path, relative_path};

const LINE_CAP: usize = 600;
const LINE_WARN: usize = 550;

pub(crate) struct WorkspaceInventory {
    crates: Vec<CrateInfo>,
    files: Vec<FileInfo>,
}

struct CrateInfo {
    member: String,
    package_name: String,
    path: String,
    has_readme: bool,
    direct_workspace_deps: Vec<String>,
    public_modules: usize,
    private_modules: usize,
    reexports: usize,
}

struct FileInfo {
    path: String,
    lines: usize,
    kind: FileKind,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum FileKind {
    Rust,
    Markdown,
    Toml,
    Hook,
    Workflow,
    Other,
}

impl WorkspaceInventory {
    pub(crate) fn collect(workspace: &Path) -> Result<Self> {
        let crates = collect_crates(workspace)?;
        let files = collect_files(workspace)?;
        Ok(Self { crates, files })
    }
}

pub(crate) fn write_crates(inventory: &WorkspaceInventory) -> String {
    let mut out = new_doc("Workspace Crates");
    out.push_str("| Member | Package | Path | README | Public mods | Private mods | Re-exports | Workspace deps |\n");
    out.push_str("|---|---|---|---|---:|---:|---:|---|\n");
    for krate in &inventory.crates {
        let readme = if krate.has_readme { "yes" } else { "missing" };
        out.push_str(&format!(
            "| `{}` | `{}` | `{}` | {} | {} | {} | {} | {} |\n",
            krate.member,
            krate.package_name,
            krate.path,
            readme,
            krate.public_modules,
            krate.private_modules,
            krate.reexports,
            format_code_list(&krate.direct_workspace_deps)
        ));
    }
    out
}

pub(crate) fn write_file_tree(inventory: &WorkspaceInventory) -> String {
    let mut out = new_doc("File Tree");
    out.push_str("Compact tree by maintained surface. Counts exclude `.docs/generated/`, `target/`, and VCS internals.\n\n");
    out.push_str("| Surface | Rust | Markdown | TOML | Other tracked inputs |\n");
    out.push_str("|---|---:|---:|---:|---:|\n");
    for prefix in [".docs", "crates", "xtask", ".githooks", ".github"] {
        let counts = count_by_kind(&inventory.files, prefix);
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} |\n",
            prefix, counts.rust, counts.markdown, counts.toml, counts.other
        ));
    }
    out.push_str("\n## Workspace Members\n\n");
    out.push_str("| Member | Source tree | Tests | Assets/docs |\n");
    out.push_str("|---|---:|---:|---:|\n");
    for krate in &inventory.crates {
        let source_prefix = format!("{}/src/", krate.path);
        let tests_prefix = format!("{}/tests/", krate.path);
        let assets_prefix = format!("{}/assets/", krate.path);
        let docs_prefix = format!("{}/README.md", krate.path);
        let source = count_prefix(&inventory.files, &source_prefix);
        let tests = count_prefix(&inventory.files, &tests_prefix);
        let assets = count_prefix(&inventory.files, &assets_prefix)
            + usize::from(inventory.files.iter().any(|f| f.path == docs_prefix));
        out.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            krate.member, source, tests, assets
        ));
    }
    out
}

pub(crate) fn write_file_health(inventory: &WorkspaceInventory) -> String {
    let mut out = new_doc("File Health");
    let source_files = inventory
        .files
        .iter()
        .filter(|file| file.kind == FileKind::Rust);
    let missing: Vec<&CrateInfo> = inventory
        .crates
        .iter()
        .filter(|krate| !krate.has_readme && krate.path.starts_with("crates/"))
        .collect();
    out.push_str("## Rust Files At Or Above 600 Lines\n\n");
    write_file_table(
        &mut out,
        source_files.clone().filter(|file| file.lines >= LINE_CAP),
    );
    out.push_str("## Rust Files At 550-599 Lines\n\n");
    write_file_table(
        &mut out,
        source_files.filter(|file| file.lines >= LINE_WARN && file.lines < LINE_CAP),
    );
    out.push_str("## Missing Crate READMEs\n\n");
    if missing.is_empty() {
        out.push_str("None.\n\n");
    } else {
        for krate in missing {
            out.push_str(&format!("- `{}/README.md`\n", krate.path));
        }
        out.push('\n');
    }
    out.push_str("## Generated Output Rules\n\n");
    out.push_str("- Every generated file must start with the standard generated header.\n");
    out.push_str("- `cargo xtask docs-check` fails when checked-in generated docs drift.\n");
    out.push_str("- Stale files under `.docs/generated/` are removed by `cargo xtask docs`.\n");
    out.push_str("- Markdown docs, TOML assets, hooks, and workflows are excluded from Rust file-health tables.\n");
    out
}

fn collect_crates(workspace: &Path) -> Result<Vec<CrateInfo>> {
    let root_manifest = read_toml(&workspace.join("Cargo.toml"))?;
    let members = root_manifest
        .get("workspace")
        .and_then(|value| value.get("members"))
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
        .map(str::to_string);
    let mut crates = Vec::new();
    for member in members {
        let member_path = workspace.join(&member);
        let manifest = read_toml(&member_path.join("Cargo.toml"))?;
        let package_name = manifest
            .get("package")
            .and_then(|value| value.get("name"))
            .and_then(toml::Value::as_str)
            .unwrap_or(&member)
            .to_string();
        let direct_workspace_deps = direct_workspace_deps(&manifest);
        let lib_path = member_path.join("src/lib.rs");
        let (public_modules, private_modules, reexports) = module_counts(&lib_path)?;
        let has_readme = member_path.join("README.md").exists();
        crates.push(CrateInfo {
            member: member_name(&member),
            package_name,
            path: member,
            has_readme,
            direct_workspace_deps,
            public_modules,
            private_modules,
            reexports,
        });
    }
    Ok(crates)
}

fn collect_files(workspace: &Path) -> Result<Vec<FileInfo>> {
    let mut files = Vec::new();
    for root in [".docs", "crates", "xtask", ".githooks", ".github"] {
        let path = workspace.join(root);
        if path.exists() {
            collect_files_under(workspace, &path, &mut files)?;
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

fn collect_files_under(workspace: &Path, dir: &Path, out: &mut Vec<FileInfo>) -> Result<()> {
    let mut entries = fs::read_dir(dir)
        .with_context(|| format!("listing {}", dir.display()))?
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("reading {}", dir.display()))?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        let relative = relative_path(workspace, &path)?;
        if should_skip(&relative) {
            continue;
        }
        if path.is_dir() {
            collect_files_under(workspace, &path, out)?;
        } else {
            let text = fs::read_to_string(&path).unwrap_or_default();
            out.push(FileInfo {
                path: relative,
                lines: line_count(&text),
                kind: file_kind(&path),
            });
        }
    }
    Ok(())
}

fn read_toml(path: &Path) -> Result<toml::Value> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

fn direct_workspace_deps(manifest: &toml::Value) -> Vec<String> {
    let mut deps = Vec::new();
    for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
        let Some(table) = manifest.get(section).and_then(toml::Value::as_table) else {
            continue;
        };
        for (name, value) in table {
            let is_workspace = value
                .as_table()
                .and_then(|table| table.get("workspace"))
                .and_then(toml::Value::as_bool)
                .unwrap_or(false);
            if is_workspace && name.starts_with("continuity-") {
                deps.push(name.clone());
            }
        }
    }
    deps.sort();
    deps.dedup();
    deps
}

fn module_counts(path: &Path) -> Result<(usize, usize, usize)> {
    if !path.exists() {
        return Ok((0, 0, 0));
    }
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut public_modules = 0;
    let mut private_modules = 0;
    let mut reexports = 0;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("pub mod ") {
            public_modules += 1;
        } else if trimmed.starts_with("pub(crate) mod ") || trimmed.starts_with("mod ") {
            private_modules += 1;
        } else if trimmed.starts_with("pub use ") {
            reexports += 1;
        }
    }
    Ok((public_modules, private_modules, reexports))
}

fn member_name(member: &str) -> String {
    member
        .rsplit_once('/')
        .map_or(member, |(_, name)| name)
        .to_string()
}

fn file_kind(path: &Path) -> FileKind {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("rs") => FileKind::Rust,
        Some("md") => FileKind::Markdown,
        Some("toml") => FileKind::Toml,
        Some("yml" | "yaml") => FileKind::Workflow,
        _ if normalize_path(path).contains(".githooks/") => FileKind::Hook,
        _ => FileKind::Other,
    }
}

fn should_skip(relative: &str) -> bool {
    relative.starts_with(".docs/generated/")
        || relative.contains("/target/")
        || relative.starts_with(".git/")
}

struct KindCounts {
    rust: usize,
    markdown: usize,
    toml: usize,
    other: usize,
}

fn count_by_kind(files: &[FileInfo], prefix: &str) -> KindCounts {
    let mut counts: BTreeMap<FileKind, usize> = BTreeMap::new();
    for file in files.iter().filter(|file| file.path.starts_with(prefix)) {
        *counts.entry(file.kind).or_default() += 1;
    }
    KindCounts {
        rust: *counts.get(&FileKind::Rust).unwrap_or(&0),
        markdown: *counts.get(&FileKind::Markdown).unwrap_or(&0),
        toml: *counts.get(&FileKind::Toml).unwrap_or(&0),
        other: files
            .iter()
            .filter(|file| file.path.starts_with(prefix))
            .filter(|file| {
                !matches!(
                    file.kind,
                    FileKind::Rust | FileKind::Markdown | FileKind::Toml
                )
            })
            .count(),
    }
}

fn count_prefix(files: &[FileInfo], prefix: &str) -> usize {
    files
        .iter()
        .filter(|file| file.path.starts_with(prefix))
        .count()
}

fn write_file_table<'a>(out: &mut String, files: impl Iterator<Item = &'a FileInfo>) {
    let rows: Vec<&FileInfo> = files.collect();
    if rows.is_empty() {
        out.push_str("None.\n\n");
        return;
    }
    out.push_str("| File | Lines |\n");
    out.push_str("|---|---:|\n");
    for file in rows {
        out.push_str(&format!(
            "| `{}` | {} |\n",
            escape_md_cell(&file.path),
            file.lines
        ));
    }
    out.push('\n');
}

fn format_code_list(items: &[String]) -> String {
    if items.is_empty() {
        return String::new();
    }
    items
        .iter()
        .map(|item| format!("`{item}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_health_ignores_non_rust_files_for_line_cap_tables() {
        let inventory = WorkspaceInventory {
            crates: Vec::new(),
            files: vec![
                FileInfo {
                    path: ".docs/development/roadmap.md".into(),
                    lines: 900,
                    kind: FileKind::Markdown,
                },
                FileInfo {
                    path: "crates/keymap/assets/default.toml".into(),
                    lines: 900,
                    kind: FileKind::Toml,
                },
                FileInfo {
                    path: "crates/ui/src/window_paint.rs".into(),
                    lines: 601,
                    kind: FileKind::Rust,
                },
            ],
        };

        let out = write_file_health(&inventory);
        assert!(out.contains("crates/ui/src/window_paint.rs"));
        assert!(!out.contains(".docs/development/roadmap.md"));
        assert!(!out.contains("crates/keymap/assets/default.toml"));
    }
}
