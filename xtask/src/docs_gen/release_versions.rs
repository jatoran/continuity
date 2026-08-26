//! Guard the hand-written release-coordinate table against version drift.
//!
//! `EMBEDDING.md` advertises the current version of each release train. Those
//! numbers were previously copied by hand, so a version bump could ship while
//! the public integration entry point still named the previous release. This
//! validator makes the table a checked surface: it re-reads both canonical
//! sources and fails `cargo xtask docs`/`docs-check` on any disagreement.

use std::fs;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};

const EMBEDDING_DOC: &str = "EMBEDDING.md";
const DESKTOP_MANIFEST: &str = "crates/app/Cargo.toml";
const SDK_MANIFEST: &str = "sdk/release.toml";

const DESKTOP_ROW_LABEL: &str = "Native Windows desktop";
const SDK_ROW_LABEL: &str = "Embeddable SDK family";

/// Fail when `EMBEDDING.md` disagrees with a canonical version source.
pub(crate) fn validate(workspace: &Path) -> Result<()> {
    let desktop_version = desktop_version(workspace)?;
    let sdk_version = sdk_version(workspace)?;

    let doc_path = workspace.join(EMBEDDING_DOC);
    let doc =
        fs::read_to_string(&doc_path).with_context(|| format!("read {}", doc_path.display()))?;

    let mut issues = Vec::new();
    check_row(
        &doc,
        DESKTOP_ROW_LABEL,
        &desktop_version,
        DESKTOP_MANIFEST,
        &mut issues,
    );
    check_row(&doc, SDK_ROW_LABEL, &sdk_version, SDK_MANIFEST, &mut issues);

    if issues.is_empty() {
        return Ok(());
    }
    bail!(
        "{EMBEDDING_DOC} release coordinates are stale:\n  {}\nUpdate the \
         `Current coordinates` table in the same change as the version bump.",
        issues.join("\n  ")
    );
}

fn check_row(doc: &str, label: &str, expected: &str, source: &str, issues: &mut Vec<String>) {
    match documented_version(doc, label) {
        None => issues.push(format!(
            "no `{label}` row found in the `Current coordinates` table"
        )),
        Some(found) if found != expected => issues.push(format!(
            "`{label}` says `{found}` but {source} says `{expected}`"
        )),
        Some(_) => {}
    }
}

/// Read the version cell out of a `| label | `version` | source |` table row.
fn documented_version(doc: &str, label: &str) -> Option<String> {
    doc.lines()
        .filter(|line| line.trim_start().starts_with('|'))
        .find_map(|line| {
            let mut cells = line.split('|').map(str::trim).skip(1);
            let found_label = cells.next()?;
            if found_label != label {
                return None;
            }
            let version = cells.next()?;
            Some(version.trim_matches('`').to_owned())
        })
}

fn desktop_version(workspace: &Path) -> Result<String> {
    let path = workspace.join(DESKTOP_MANIFEST);
    let contents = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let manifest: toml::Value = contents
        .parse()
        .with_context(|| format!("parse {}", path.display()))?;
    manifest
        .get("package")
        .and_then(|package| package.get("version"))
        .and_then(|version| version.as_str())
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("{DESKTOP_MANIFEST} has no package.version"))
}

fn sdk_version(workspace: &Path) -> Result<String> {
    let path = workspace.join(SDK_MANIFEST);
    let contents = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let manifest: toml::Value = contents
        .parse()
        .with_context(|| format!("parse {}", path.display()))?;
    manifest
        .get("sdk")
        .and_then(|sdk| sdk.get("version"))
        .and_then(|version| version.as_str())
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("{SDK_MANIFEST} has no sdk.version"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = "\
| Release train | Current version | Canonical source |
|---|---:|---|
| Native Windows desktop | `0.4.8` | `crates/app/Cargo.toml` |
| Embeddable SDK family | `0.2.36` | `sdk/release.toml` |
";

    #[test]
    fn documented_version_reads_the_version_cell() {
        assert_eq!(
            documented_version(TABLE, DESKTOP_ROW_LABEL).as_deref(),
            Some("0.4.8")
        );
        assert_eq!(
            documented_version(TABLE, SDK_ROW_LABEL).as_deref(),
            Some("0.2.36")
        );
    }

    #[test]
    fn documented_version_ignores_unrelated_rows() {
        assert_eq!(documented_version(TABLE, "Nonexistent train"), None);
    }

    #[test]
    fn check_row_reports_a_stale_version() {
        let mut issues = Vec::new();
        check_row(
            TABLE,
            DESKTOP_ROW_LABEL,
            "0.4.9",
            DESKTOP_MANIFEST,
            &mut issues,
        );
        assert_eq!(issues.len(), 1);
        assert!(issues[0].contains("`0.4.8`"), "{}", issues[0]);
        assert!(issues[0].contains("`0.4.9`"), "{}", issues[0]);
    }

    #[test]
    fn check_row_accepts_a_matching_version() {
        let mut issues = Vec::new();
        check_row(
            TABLE,
            DESKTOP_ROW_LABEL,
            "0.4.8",
            DESKTOP_MANIFEST,
            &mut issues,
        );
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn check_row_reports_a_missing_row() {
        let mut issues = Vec::new();
        check_row(
            TABLE,
            "Missing train",
            "1.0.0",
            DESKTOP_MANIFEST,
            &mut issues,
        );
        assert_eq!(issues.len(), 1);
        assert!(
            issues[0].contains("no `Missing train` row"),
            "{}",
            issues[0]
        );
    }
}
