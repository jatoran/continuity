//! Per-crate module generated docs.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;

use crate::docs_gen::rust_source::{crate_rust_sources, first_doc_line, WorkspaceCrate};
use crate::docs_gen::{escape_md_cell, new_doc};

#[derive(Clone, Debug)]
pub(crate) struct ModuleDoc {
    pub(crate) module_path: String,
    pub(crate) visibility: String,
    pub(crate) source_path: String,
    pub(crate) lines: usize,
    pub(crate) first_doc_line: String,
}

pub(crate) fn write_modules(workspace: &Path, krate: &WorkspaceCrate) -> Result<String> {
    let modules = collect_modules(workspace, krate)?;
    let mut out = new_doc(&format!("Modules: {}", krate.member));
    out.push_str(&format!("Generated from `{}/src/**/*.rs`.\n\n", krate.path));
    out.push_str(&format!("- Package: `{}`\n\n", krate.package_name));
    out.push_str("| Module | Visibility | Source | Lines | First doc line |\n");
    out.push_str("|---|---|---|---:|---|\n");
    for module in modules {
        out.push_str(&format!(
            "| `{}` | {} | `{}` | {} | {} |\n",
            module.module_path,
            module.visibility,
            module.source_path,
            module.lines,
            escape_md_cell(&module.first_doc_line)
        ));
    }
    Ok(out)
}

pub(crate) fn collect_modules(workspace: &Path, krate: &WorkspaceCrate) -> Result<Vec<ModuleDoc>> {
    let sources = crate_rust_sources(workspace, krate)?;
    let visibilities = module_visibilities(&sources, &krate.path);
    Ok(sources
        .into_iter()
        .map(|source| {
            let visibility = if source.module_path == "crate" {
                "root".to_string()
            } else {
                visibilities
                    .get(&source.relative)
                    .cloned()
                    .unwrap_or_else(|| "private".into())
            };
            ModuleDoc {
                module_path: source.module_path,
                visibility,
                source_path: source.relative,
                lines: source.lines,
                first_doc_line: first_doc_line(&source.text),
            }
        })
        .collect())
}

fn module_visibilities(
    sources: &[crate::docs_gen::rust_source::RustSource],
    crate_path: &str,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for source in sources {
        let source_dir = Path::new(&source.relative)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let child_prefix =
            if source.relative.ends_with("lib.rs") || source.relative.ends_with("main.rs") {
                source_dir
            } else {
                let stem = Path::new(&source.relative)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                source_dir.join(stem)
            };
        for raw in source.text.lines() {
            let trimmed = raw.trim_start();
            if let Some((visibility, name)) = parse_mod_decl(trimmed) {
                let child = format!(
                    "{}/{}.rs",
                    child_prefix.to_string_lossy().replace('\\', "/"),
                    name
                );
                if child.starts_with(crate_path) {
                    out.insert(child, visibility.to_string());
                }
            }
        }
    }
    out
}

fn parse_mod_decl(line: &str) -> Option<(&'static str, String)> {
    let (visibility, rest) = if let Some(rest) = line.strip_prefix("pub mod ") {
        ("public", rest)
    } else if let Some(rest) = line.strip_prefix("pub(crate) mod ") {
        ("crate", rest)
    } else if let Some(rest) = line.strip_prefix("mod ") {
        ("private", rest)
    } else {
        return None;
    };
    let name = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect::<String>();
    (!name.is_empty()).then_some((visibility, name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docs_gen::rust_source::RustSource;

    #[test]
    fn maps_nested_mod_declarations_to_no_mod_rs_paths() {
        let sources = vec![RustSource {
            relative: "crates/demo/src/lib.rs".into(),
            module_path: "crate".into(),
            text: "pub mod visible;\nmod hidden;\n".into(),
            lines: 2,
        }];
        let map = module_visibilities(&sources, "crates/demo");
        assert_eq!(map["crates/demo/src/visible.rs"], "public");
        assert_eq!(map["crates/demo/src/hidden.rs"], "private");
    }
}
