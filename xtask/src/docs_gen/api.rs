//! Per-crate public API generated docs.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;

use crate::docs_gen::rust_source::{
    crate_rust_sources, parse_public_items, PublicItem, WorkspaceCrate,
};
use crate::docs_gen::{escape_md_cell, new_doc};

pub(crate) fn write_api(workspace: &Path, krate: &WorkspaceCrate) -> Result<String> {
    let items = collect_api_items(workspace, krate)?;
    let mut by_kind: BTreeMap<String, Vec<PublicItem>> = BTreeMap::new();
    for item in items {
        by_kind.entry(item.kind.clone()).or_default().push(item);
    }

    let mut out = new_doc(&format!("Public API: {}", krate.member));
    out.push_str(&format!(
        "Generated from top-level public items in `{}/src/**/*.rs`.\n",
        krate.path
    ));
    out.push_str(&format!("- Package: `{}`\n", krate.package_name));
    out.push_str("Impl methods are omitted; start from the owning type's source path.\n\n");
    for (kind, items) in by_kind {
        out.push_str(&format!("## `{kind}`\n\n"));
        out.push_str("| Name | Source | Signature | First doc line |\n");
        out.push_str("|---|---|---|---|\n");
        for item in items {
            out.push_str(&format!(
                "| `{}` | `{}`:{} | `{}` | {} |\n",
                escape_md_cell(&item.name),
                item.path,
                item.line,
                escape_md_cell(&item.signature),
                escape_md_cell(&item.doc)
            ));
        }
        out.push('\n');
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    Ok(out)
}

pub(crate) fn collect_api_items(
    workspace: &Path,
    krate: &WorkspaceCrate,
) -> Result<Vec<PublicItem>> {
    let mut items = Vec::new();
    for source in crate_rust_sources(workspace, krate)? {
        items.extend(parse_public_items(&source));
    }
    items.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.line.cmp(&b.line))
    });
    Ok(items)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::write_api;
    use crate::docs_gen::rust_source::WorkspaceCrate;
    use crate::docs_gen::rust_source::{parse_public_items, RustSource};

    #[test]
    fn api_omits_impl_methods() {
        let source = RustSource {
            relative: "crates/demo/src/lib.rs".into(),
            module_path: "crate".into(),
            text: "pub struct Demo;\nimpl Demo { pub fn method(&self) {} }\n".into(),
            lines: 2,
        };
        let items = parse_public_items(&source);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "Demo");
    }

    #[test]
    fn api_output_ends_with_one_newline() {
        let workspace = tempdir().expect("temporary workspace");
        let source_dir = workspace.path().join("crates/demo/src");
        fs::create_dir_all(&source_dir).expect("create demo source directory");
        fs::write(source_dir.join("lib.rs"), "pub struct Demo;\n").expect("write demo source");
        let krate = WorkspaceCrate {
            member: "demo".into(),
            package_name: "continuity-demo".into(),
            path: "crates/demo".into(),
        };

        let output = write_api(workspace.path(), &krate).expect("generate API inventory");

        assert!(output.ends_with('\n'));
        assert!(!output.ends_with("\n\n"));
    }
}
