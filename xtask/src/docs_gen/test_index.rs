//! Test inventory generated docs.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::docs_gen::{escape_md_cell, new_doc, relative_path};

pub(crate) fn write_test_index(workspace: &Path) -> Result<String> {
    let crates = collect_crate_tests(workspace)?;
    let mut out = new_doc("Test Index");
    out.push_str("Generated from `crates/*/src`, `crates/*/tests`, and `crates/*/benches`.\n\n");
    write_summary(&mut out, &crates);
    write_integration_files(&mut out, &crates);
    write_special_suites(&mut out, &crates);
    Ok(out)
}

pub(crate) struct CrateTests {
    pub(crate) member: String,
    pub(crate) package_name: String,
    pub(crate) unit_test_files: usize,
    pub(crate) unit_test_functions: usize,
    pub(crate) ignored_tests: usize,
    pub(crate) integration_files: Vec<TestFile>,
    pub(crate) bench_files: Vec<TestFile>,
}

#[derive(Clone)]
pub(crate) struct TestFile {
    pub(crate) path: String,
    pub(crate) stem: String,
    pub(crate) kind: TestKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TestKind {
    Integration,
    Bench,
    PerfGate,
    E2e,
    Smoke,
    Canary,
    Golden,
}

pub(crate) fn collect_crate_tests(workspace: &Path) -> Result<Vec<CrateTests>> {
    let members = workspace_members(workspace)?;
    let mut crates = Vec::new();
    for member in members {
        let crate_path = workspace.join(&member);
        if !member.starts_with("crates/") {
            continue;
        }
        let package_name = package_name(&crate_path.join("Cargo.toml"))?;
        let (unit_test_files, unit_test_functions, ignored_tests) =
            unit_test_counts(&crate_path.join("src"))?;
        let integration_files =
            collect_test_files(workspace, &crate_path.join("tests"), TestKind::Integration)?;
        let bench_files =
            collect_test_files(workspace, &crate_path.join("benches"), TestKind::Bench)?;
        crates.push(CrateTests {
            member: member_name(&member),
            package_name,
            unit_test_files,
            unit_test_functions,
            ignored_tests,
            integration_files,
            bench_files,
        });
    }
    crates.sort_by(|a, b| a.member.cmp(&b.member));
    Ok(crates)
}

fn write_summary(out: &mut String, crates: &[CrateTests]) {
    out.push_str("## Summary\n\n");
    out.push_str("| Crate | Unit files | Unit tests | Ignored tests | Integration files | Bench files | Command hint |\n");
    out.push_str("|---|---:|---:|---:|---:|---:|---|\n");
    for krate in crates {
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {} | `cargo test -p {}` |\n",
            krate.member,
            krate.unit_test_files,
            krate.unit_test_functions,
            krate.ignored_tests,
            krate.integration_files.len(),
            krate.bench_files.len(),
            krate.package_name
        ));
    }
    out.push('\n');
}

fn write_integration_files(out: &mut String, crates: &[CrateTests]) {
    out.push_str("## Integration And Bench Files\n\n");
    out.push_str("| Crate | Integration tests | Bench files |\n");
    out.push_str("|---|---|---|\n");
    for krate in crates {
        out.push_str(&format!(
            "| `{}` | {} | {} |\n",
            krate.member,
            format_file_list(&krate.integration_files, &krate.package_name),
            format_file_list(&krate.bench_files, &krate.package_name)
        ));
    }
    out.push('\n');
}

fn write_special_suites(out: &mut String, crates: &[CrateTests]) {
    let mut special = Vec::new();
    for krate in crates {
        for file in krate
            .integration_files
            .iter()
            .chain(krate.bench_files.iter())
        {
            if file.kind != TestKind::Integration && file.kind != TestKind::Bench {
                special.push((krate, file));
            }
        }
    }
    out.push_str("## Special Suites\n\n");
    if special.is_empty() {
        out.push_str("None.\n");
        return;
    }
    out.push_str("| Kind | File | Command hint |\n");
    out.push_str("|---|---|---|\n");
    for (krate, file) in special {
        out.push_str(&format!(
            "| {} | `{}` | `{}` |\n",
            file.kind.as_str(),
            escape_md_cell(&file.path),
            command_for_file(krate, file)
        ));
    }
}

fn workspace_members(workspace: &Path) -> Result<Vec<String>> {
    let path = workspace.join("Cargo.toml");
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let parsed: toml::Value =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let members = parsed
        .get("workspace")
        .and_then(|value| value.get("members"))
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
        .map(str::to_string)
        .collect();
    Ok(members)
}

fn package_name(path: &Path) -> Result<String> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let parsed: toml::Value =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(parsed
        .get("package")
        .and_then(|value| value.get("name"))
        .and_then(toml::Value::as_str)
        .unwrap_or("")
        .to_string())
}

fn unit_test_counts(src_dir: &Path) -> Result<(usize, usize, usize)> {
    let mut files = Vec::new();
    collect_rs_files(src_dir, &mut files)?;
    let mut unit_files = 0;
    let mut test_functions = 0;
    let mut ignored = 0;
    for path in files {
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        if text.contains("#[cfg(test)]") {
            unit_files += 1;
            test_functions += text.matches("#[test]").count();
            ignored += text.matches("#[ignore").count();
        }
    }
    Ok((unit_files, test_functions, ignored))
}

fn collect_test_files(
    workspace: &Path,
    dir: &Path,
    default_kind: TestKind,
) -> Result<Vec<TestFile>> {
    let mut files = Vec::new();
    collect_rs_files(dir, &mut files)?;
    let mut out = Vec::new();
    for path in files {
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("")
            .to_string();
        let relative = relative_path(workspace, &path)?;
        out.push(TestFile {
            kind: classify_test_file(&stem, default_kind),
            path: relative,
            stem,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    let mut entries = fs::read_dir(dir)
        .with_context(|| format!("listing {}", dir.display()))?
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("reading {}", dir.display()))?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

fn classify_test_file(stem: &str, default_kind: TestKind) -> TestKind {
    if stem.contains("perf_gate") || stem == "perf_gates" {
        TestKind::PerfGate
    } else if stem.contains("e2e") {
        TestKind::E2e
    } else if stem.contains("smoke") {
        TestKind::Smoke
    } else if stem.contains("canary") {
        TestKind::Canary
    } else if stem.contains("golden") {
        TestKind::Golden
    } else {
        default_kind
    }
}

fn format_file_list(files: &[TestFile], package_name: &str) -> String {
    if files.is_empty() {
        return String::new();
    }
    files
        .iter()
        .map(|file| format!("`{}`", command_for_file_by_package(package_name, file)))
        .collect::<Vec<_>>()
        .join("<br>")
}

fn command_for_file(krate: &CrateTests, file: &TestFile) -> String {
    command_for_file_by_package(&krate.package_name, file)
}

fn command_for_file_by_package(package_name: &str, file: &TestFile) -> String {
    match file.kind {
        TestKind::Bench => format!("cargo bench -p {} --bench {}", package_name, file.stem),
        _ => format!("cargo test -p {} --test {}", package_name, file.stem),
    }
}

fn member_name(member: &str) -> String {
    member
        .rsplit_once('/')
        .map_or(member, |(_, name)| name)
        .to_string()
}

impl TestKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Integration => "integration",
            Self::Bench => "bench",
            Self::PerfGate => "perf gate",
            Self::E2e => "e2e",
            Self::Smoke => "smoke",
            Self::Canary => "canary",
            Self::Golden => "golden",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_special_test_files() {
        assert!(matches!(
            classify_test_file("e2e_smoke", TestKind::Integration),
            TestKind::E2e
        ));
        assert!(matches!(
            classify_test_file("pixel_canary", TestKind::Integration),
            TestKind::Canary
        ));
        assert!(matches!(
            classify_test_file("perf_gates", TestKind::Integration),
            TestKind::PerfGate
        ));
    }

    #[test]
    fn bench_command_uses_cargo_bench() {
        let file = TestFile {
            path: "crates/buffer/benches/edit.rs".into(),
            stem: "edit".into(),
            kind: TestKind::Bench,
        };
        assert_eq!(
            command_for_file_by_package("continuity-buffer", &file),
            "cargo bench -p continuity-buffer --bench edit"
        );
    }
}
