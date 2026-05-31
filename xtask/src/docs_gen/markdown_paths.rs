//! Conservative Markdown route/path validation for handwritten docs.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::docs_gen::relative_path;

/// Validate path-like Markdown links and inline-code routes.
pub(crate) fn validate(workspace: &Path) -> Result<()> {
    let mut issues = Vec::new();
    validate_root_agent_mirror(workspace, &mut issues)?;
    for path in markdown_files_to_validate(workspace)? {
        validate_markdown_file(workspace, &path, &mut issues)?;
    }
    if !issues.is_empty() {
        let mut message = String::from("markdown route/path validation failed:");
        for issue in issues {
            message.push_str("\n- ");
            message.push_str(&issue);
        }
        bail!("{message}");
    }
    Ok(())
}

fn validate_root_agent_mirror(workspace: &Path, issues: &mut Vec<String>) -> Result<()> {
    let agents = workspace.join("AGENTS.md");
    let claude = workspace.join("CLAUDE.md");
    if !agents.exists() || !claude.exists() {
        return Ok(());
    }
    let agents_text =
        fs::read_to_string(&agents).with_context(|| format!("reading {}", agents.display()))?;
    let claude_text =
        fs::read_to_string(&claude).with_context(|| format!("reading {}", claude.display()))?;
    if agents_text != claude_text {
        issues.push("AGENTS.md and CLAUDE.md are both present but not byte-identical".into());
    }
    Ok(())
}

fn markdown_files_to_validate(workspace: &Path) -> Result<Vec<PathBuf>> {
    let mut files = BTreeSet::new();
    for relative in [
        "AGENTS.md",
        "CLAUDE.md",
        ".docs/AGENTS.md",
        ".docs/CLAUDE.md",
        ".docs/development/generated_documentation_plan.md",
    ] {
        let path = workspace.join(relative);
        if path.exists() {
            files.insert(path);
        }
    }
    for relative in [".docs/design", ".docs/technical"] {
        let path = workspace.join(relative);
        if path.exists() {
            collect_markdown_files(&path, &mut files)?;
        }
    }
    Ok(files.into_iter().collect())
}

fn collect_markdown_files(dir: &Path, out: &mut BTreeSet<PathBuf>) -> Result<()> {
    let mut entries = fs::read_dir(dir)
        .with_context(|| format!("listing {}", dir.display()))?
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("reading {}", dir.display()))?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_markdown_files(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "md") {
            out.insert(path);
        }
    }
    Ok(())
}

fn validate_markdown_file(workspace: &Path, path: &Path, issues: &mut Vec<String>) -> Result<()> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let relative = relative_path(workspace, path)?;
    let mut in_fence = false;
    for (idx, line) in text.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let line_number = idx + 1;
        for raw in markdown_links(line) {
            if let Some(candidate) = normalize_candidate(&raw, CandidateKind::MarkdownLink) {
                validate_candidate(workspace, path, &relative, line_number, candidate, issues);
            }
        }
        for raw in inline_code_spans(line) {
            if let Some(candidate) = normalize_candidate(&raw, CandidateKind::InlineCode) {
                validate_candidate(workspace, path, &relative, line_number, candidate, issues);
            }
        }
    }
    Ok(())
}

fn validate_candidate(
    workspace: &Path,
    doc_path: &Path,
    doc_relative: &str,
    line_number: usize,
    candidate: Candidate,
    issues: &mut Vec<String>,
) {
    let resolved = resolve_candidate(workspace, doc_path, &candidate.path);
    if !resolved.exists() {
        issues.push(format!(
            "{}:{} references missing {} `{}`",
            doc_relative,
            line_number,
            candidate.kind.label(),
            candidate.path
        ));
    }
}

#[derive(Clone, Copy)]
enum CandidateKind {
    MarkdownLink,
    InlineCode,
}

impl CandidateKind {
    fn label(self) -> &'static str {
        match self {
            Self::MarkdownLink => "Markdown link",
            Self::InlineCode => "inline-code path",
        }
    }
}

struct Candidate {
    path: String,
    kind: CandidateKind,
}

fn normalize_candidate(raw: &str, kind: CandidateKind) -> Option<Candidate> {
    let stripped = strip_markdown_target(raw)?;
    if should_skip_path_like_text(&stripped) {
        return None;
    }
    if matches!(kind, CandidateKind::InlineCode) && !is_inline_path_like(&stripped) {
        return None;
    }
    if matches!(kind, CandidateKind::MarkdownLink) && !is_markdown_link_path_like(&stripped) {
        return None;
    }
    Some(Candidate {
        path: stripped,
        kind,
    })
}

fn strip_markdown_target(raw: &str) -> Option<String> {
    let mut s = raw.trim().trim_matches('<').trim_matches('>').trim();
    if s.is_empty()
        || s.starts_with('#')
        || s.starts_with("http://")
        || s.starts_with("https://")
        || s.starts_with("mailto:")
    {
        return None;
    }
    if let Some((before, _)) = s.split_once('#') {
        s = before.trim();
    }
    if let Some((before, _)) = s.split_once('?') {
        s = before.trim();
    }
    let s = strip_rust_symbol_suffix(strip_line_suffix(s))
        .trim_start_matches('(')
        .trim_end_matches(['.', ',', ';', ':', ')'])
        .replace('\\', "/");
    (!s.is_empty()).then_some(s)
}

fn strip_line_suffix(s: &str) -> &str {
    let Some((path, suffix)) = s.rsplit_once(':') else {
        return s;
    };
    if suffix.chars().all(|c| c.is_ascii_digit()) {
        path
    } else {
        s
    }
}

fn strip_rust_symbol_suffix(s: &str) -> &str {
    let Some(index) = s.find(".rs::") else {
        return s;
    };
    &s[..index + ".rs".len()]
}

fn should_skip_path_like_text(s: &str) -> bool {
    s.contains('<')
        || s.contains('>')
        || s.contains('*')
        || s.contains('{')
        || s.contains('}')
        || s.contains('$')
        || s.contains('…')
        || s.contains(' ')
        || s.contains("://")
        || s.starts_with("target/")
        || s.starts_with("./target/")
        || s.starts_with("example/")
        || s.starts_with("path/to/")
}

fn is_inline_path_like(s: &str) -> bool {
    has_repo_prefix(s) || is_root_file(s) || s.starts_with("../") || s.starts_with("./")
}

fn is_markdown_link_path_like(s: &str) -> bool {
    has_repo_prefix(s)
        || is_root_file(s)
        || s.starts_with("../")
        || s.starts_with("./")
        || s.ends_with('/')
        || has_known_path_extension(s)
}

fn has_repo_prefix(s: &str) -> bool {
    [
        ".docs/",
        "crates/",
        "xtask/",
        ".githooks/",
        ".github/",
        ".cargo/",
        ".perf/",
        "scripts/",
    ]
    .iter()
    .any(|prefix| s.starts_with(prefix))
}

fn is_root_file(s: &str) -> bool {
    matches!(
        s,
        "AGENTS.md"
            | "CLAUDE.md"
            | "Cargo.toml"
            | "Cargo.lock"
            | "clippy.toml"
            | "deny.toml"
            | "rust-toolchain.toml"
            | "rustfmt.toml"
    )
}

fn has_known_path_extension(s: &str) -> bool {
    Path::new(s)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext,
                "md" | "rs" | "toml" | "yml" | "yaml" | "json" | "jsonl" | "tsv"
            )
        })
}

fn resolve_candidate(workspace: &Path, doc_path: &Path, candidate: &str) -> PathBuf {
    if has_repo_prefix(candidate) || is_root_file(candidate) {
        workspace.join(candidate)
    } else {
        doc_path
            .parent()
            .unwrap_or(workspace)
            .join(candidate)
            .components()
            .collect()
    }
}

fn markdown_links(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find("](") {
        let after = &rest[start + 2..];
        let Some(end) = after.find(')') else {
            break;
        };
        out.push(after[..end].to_string());
        rest = &after[end + 1..];
    }
    out
}

fn inline_code_spans(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find('`') {
        if rest[start..].starts_with("```") {
            break;
        }
        let after = &rest[start + 1..];
        let Some(end) = after.find('`') else {
            break;
        };
        out.push(after[..end].to_string());
        rest = &after[end + 1..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_candidates_skip_placeholders_and_commands() {
        assert!(normalize_candidate("cargo xtask docs-check", CandidateKind::InlineCode).is_none());
        assert!(
            normalize_candidate(".docs/generated/api/<crate>.md", CandidateKind::InlineCode)
                .is_none()
        );
        assert!(
            normalize_candidate(".docs/generated/MESSAGES.md", CandidateKind::InlineCode).is_some()
        );
    }

    #[test]
    fn markdown_links_strip_anchor_and_line_suffix() {
        let candidate = normalize_candidate(
            "../technical/00_INDEX.md#generated",
            CandidateKind::MarkdownLink,
        )
        .expect("candidate");
        assert_eq!(candidate.path, "../technical/00_INDEX.md");
        let candidate =
            normalize_candidate("crates/core/src/message.rs:75", CandidateKind::MarkdownLink)
                .expect("candidate");
        assert_eq!(candidate.path, "crates/core/src/message.rs");
        let candidate = normalize_candidate(
            "crates/ui/src/window.rs::wndproc",
            CandidateKind::InlineCode,
        )
        .expect("candidate");
        assert_eq!(candidate.path, "crates/ui/src/window.rs");
        assert!(
            normalize_candidate("./target/release/continuity.exe", CandidateKind::InlineCode)
                .is_none()
        );
    }

    #[test]
    fn extracts_links_and_inline_code() {
        assert_eq!(
            markdown_links("- [x](../generated/README.md)"),
            vec!["../generated/README.md"]
        );
        assert_eq!(
            inline_code_spans("read `.docs/CLAUDE.md` and `Cargo.toml`"),
            vec![".docs/CLAUDE.md", "Cargo.toml"]
        );
    }
}
