//! Convention checks for portable SDK source files outside Rust crates.

use std::path::Path;

use anyhow::{Context, Result};

use crate::conventions::{Violation, FILE_LENGTH_CAP};

/// Return whether a tracked path is a portable JavaScript, TypeScript, or Python source file.
pub(crate) fn is_source(path: &str) -> bool {
    (path.starts_with("packages/") || path.starts_with("bindings/") || path.starts_with("apps/"))
        && [".js", ".mjs", ".cjs", ".jsx", ".ts", ".tsx", ".py"]
            .iter()
            .any(|extension| path.ends_with(extension))
}

/// Enforce the repository line cap for a portable source file.
pub(crate) fn check_file_length(
    path_str: &str,
    path: &Path,
    out: &mut Vec<Violation>,
) -> Result<()> {
    let content = std::fs::read_to_string(path).with_context(|| format!("reading {path_str}"))?;
    let line_count = content.lines().count();
    if line_count > FILE_LENGTH_CAP {
        out.push(Violation::new(
            "conventions:file-length",
            path_str,
            None,
            format!("{line_count} lines; max is {FILE_LENGTH_CAP}"),
        ));
    }
    Ok(())
}
