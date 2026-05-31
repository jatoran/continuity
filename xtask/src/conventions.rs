//! Repository convention checks.
//!
//! Invoked via `cargo xtask conventions`. Each rule prints
//! `conventions:<rule-name> <path>[:<line>] <message>` for any violation and
//! the command returns a non-zero exit when at least one rule fires. The
//! intent is that humans and agents run a single command before finalizing
//! work and that Git hooks (`.githooks/`) call into the same checks.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};

use crate::scan::{comment_text, find_word, strip_strings_and_comment};

/// Hard line cap per the conventions doc. Files at or near the cap are a
/// refactor signal, not a comment-density signal. The rule is
/// unconditional — there is no per-file exemption mechanism. When a file
/// crosses the cap, split it into responsibility-scoped siblings
/// (`foo.rs` + `foo/<helper>.rs`, no `mod.rs`).
const FILE_LENGTH_CAP: usize = 600;

/// Path prefixes where `anyhow` is permitted. Everything else is denied.
const ANYHOW_ALLOWED_PREFIXES: &[&str] = &[
    // The binary collapses every per-crate `thiserror` enum at the top.
    "crates/app/src/main.rs",
    // Workspace task runner; `anyhow` is the conventional shape for short
    // CLI glue and matches the existing xtask style.
    "xtask/",
];

#[derive(Debug)]
pub(crate) struct Violation {
    pub(crate) rule: &'static str,
    pub(crate) path: String,
    pub(crate) line: Option<usize>,
    pub(crate) message: String,
}

impl Violation {
    pub(crate) fn new(
        rule: &'static str,
        path: &str,
        line: Option<usize>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            rule,
            path: path.into(),
            line,
            message: message.into(),
        }
    }

    fn print(&self) {
        match self.line {
            Some(n) => eprintln!("{} {}:{} {}", self.rule, self.path, n, self.message),
            None => eprintln!("{} {} {}", self.rule, self.path, self.message),
        }
    }
}

/// Run every convention check across the workspace and return `Err` if any
/// violation surfaced. The exit status of the running process maps directly
/// to this `Result` in `main.rs`.
pub fn run() -> Result<()> {
    let files = git_tracked_files()?;
    let mut violations: Vec<Violation> = Vec::new();

    check_no_mod_rs(&files, &mut violations);
    check_no_phase_prefixed_filename(&files, &mut violations);
    check_feature_doc_intro(&files, &mut violations);
    check_no_tokio_in_lockfile(&mut violations)?;

    let mut rs_count = 0_usize;
    for path in &files {
        let path_str = normalize_path(path);
        if !path_str.ends_with(".rs") {
            continue;
        }
        rs_count += 1;
        check_rust_file(&path_str, path, &mut violations)?;
    }

    violations.sort_by(|a, b| {
        (a.rule, a.path.as_str(), a.line.unwrap_or(0)).cmp(&(
            b.rule,
            b.path.as_str(),
            b.line.unwrap_or(0),
        ))
    });

    for v in &violations {
        v.print();
    }

    // Tutorial drift — the generated asset must match what the
    // generator would produce against the current feature docs +
    // keymap. Run independently of rust-file violations so a stale
    // tutorial is always reported even alongside other findings. Fix
    // is always `cargo xtask gen-tutorial`.
    let tutorial_drift = crate::tutorial_gen::check_drift();
    if let Err(e) = &tutorial_drift {
        eprintln!("conventions:tutorial-drift {e}");
    }

    if !violations.is_empty() || tutorial_drift.is_err() {
        bail!(
            "conventions: {} rust violation(s){}",
            violations.len(),
            if tutorial_drift.is_err() {
                " + tutorial drift"
            } else {
                ""
            }
        );
    }

    println!(
        "conventions: ok ({} rust files scanned, {} tracked files total, tutorial in sync)",
        rs_count,
        files.len()
    );
    Ok(())
}

/// Wire `core.hooksPath` to the checked-in `.githooks/` directory so that the
/// hooks ship with the repo and ride the same review process as code.
pub fn install_hooks() -> Result<()> {
    let status = Command::new("git")
        .args(["config", "core.hooksPath", ".githooks"])
        .status()
        .context("running `git config core.hooksPath .githooks`")?;
    if !status.success() {
        bail!("git config failed (exit {:?})", status.code());
    }
    println!("conventions: git hooks installed (core.hooksPath = .githooks)");
    Ok(())
}

/// Validate the commit message at `path` against the project's Conventional
/// Commits envelope. Wired up by `.githooks/commit-msg`.
pub fn check_commit_msg(path: &Path) -> Result<()> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("reading commit message at {}", path.display()))?;
    let first_line = content
        .lines()
        .find(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .unwrap_or("")
        .trim_end();

    if first_line.starts_with("Merge ") || first_line.starts_with("Revert ") {
        return Ok(());
    }
    if first_line.is_empty() {
        bail!("commit-msg: empty commit message");
    }
    if matches_conventional(first_line) {
        return Ok(());
    }
    eprintln!(
        "commit-msg: first line must follow Conventional Commits.\n  saw:    {first_line}\n  expect: <type>(<scope>)?: <subject>\n  types:  feat fix docs test chore refactor perf build ci style revert"
    );
    bail!("commit-msg: invalid commit message")
}

fn matches_conventional(line: &str) -> bool {
    const TYPES: &[&str] = &[
        "feat", "fix", "docs", "test", "chore", "refactor", "perf", "build", "ci", "style",
        "revert",
    ];
    let Some(colon) = line.find(": ") else {
        return false;
    };
    let head = &line[..colon];
    let subject = line[colon + 2..].trim();
    if subject.is_empty() {
        return false;
    }
    let kind = match head.find('(') {
        Some(open) => {
            if !head.ends_with(')') {
                return false;
            }
            let scope = &head[open + 1..head.len() - 1];
            if scope.is_empty()
                || !scope
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
            {
                return false;
            }
            &head[..open]
        }
        None => head,
    };
    TYPES.contains(&kind)
}

fn git_tracked_files() -> Result<Vec<PathBuf>> {
    let output = Command::new("git")
        .args(["ls-files"])
        .output()
        .context("running `git ls-files`")?;
    if !output.status.success() {
        bail!("git ls-files failed (exit {:?})", output.status.code());
    }
    let text = std::str::from_utf8(&output.stdout)
        .map_err(|e| anyhow!("git ls-files output not utf-8: {e}"))?;
    let mut files: Vec<PathBuf> = text
        .lines()
        .map(|l| PathBuf::from(l.trim_end()))
        .filter(|p| !p.as_os_str().is_empty())
        .filter(|p| p.exists())
        .collect();
    files.sort();
    Ok(files)
}

fn normalize_path(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn check_no_mod_rs(files: &[PathBuf], out: &mut Vec<Violation>) {
    for f in files {
        let s = normalize_path(f);
        if s == "mod.rs" || s.ends_with("/mod.rs") {
            out.push(Violation {
                rule: "conventions:no-mod-rs",
                path: s,
                line: None,
                message: "mod.rs files are forbidden — use foo.rs + foo/ layout".into(),
            });
        }
    }
}

fn check_feature_doc_intro(files: &[PathBuf], out: &mut Vec<Violation>) {
    for f in files {
        let s = normalize_path(f);
        if !s.starts_with(".docs/design/features/") || !s.ends_with(".md") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(f) else {
            continue;
        };
        let mut lines = content.lines();
        let Some(_h1) = lines.find(|l| l.starts_with("# ")) else {
            out.push(Violation {
                rule: "conventions:feature-doc-intro",
                path: s,
                line: None,
                message: "feature doc has no H1".into(),
            });
            continue;
        };
        let mut intro = String::new();
        for line in lines {
            if line.starts_with("## ") || line.starts_with("# ") {
                break;
            }
            intro.push_str(line);
            intro.push('\n');
        }
        if intro.trim().is_empty() {
            out.push(Violation {
                rule: "conventions:feature-doc-intro",
                path: s,
                line: None,
                message: "feature doc must have at least one paragraph of prose between the H1 \
                     and the first H2 — this is what `cargo xtask gen-tutorial` extracts \
                     into the tutorial's per-feature section"
                    .into(),
            });
        }
    }
}

fn check_no_phase_prefixed_filename(files: &[PathBuf], out: &mut Vec<Violation>) {
    for f in files {
        let s = normalize_path(f);
        if !is_in_crates_src_or_tests(&s) {
            continue;
        }
        let basename = match s.rsplit_once('/') {
            Some((_, b)) => b,
            None => s.as_str(),
        };
        if is_phase_prefixed_filename(basename) {
            out.push(Violation {
                rule: "conventions:no-phase-prefixed-filename",
                path: s,
                line: None,
                message: "phase-prefixed filenames are forbidden \
— file names describe responsibility, not history (Phase 17.8 \u{00A7}B1). \
Pick a topic-descriptive name (e.g. `window_clipboard.rs`, not `window_phase16.rs`)."
                    .into(),
            });
        }
    }
}

fn is_in_crates_src_or_tests(path_str_norm: &str) -> bool {
    let Some(rest) = path_str_norm.strip_prefix("crates/") else {
        return false;
    };
    let Some(slash) = rest.find('/') else {
        return false;
    };
    let after_crate = &rest[slash + 1..];
    after_crate.starts_with("src/") || after_crate.starts_with("tests/")
}

fn is_phase_prefixed_filename(basename: &str) -> bool {
    let Some(stem) = basename.strip_suffix(".rs") else {
        return false;
    };
    let core = stem.strip_prefix("window_").unwrap_or(stem);
    if let Some(rest) = core.strip_prefix("phase") {
        return is_phase_filename_tail(rest);
    }
    let mut cursor = core;
    while let Some(idx) = cursor.find("_phase") {
        let rest = &cursor[idx + "_phase".len()..];
        if is_phase_filename_tail(rest) {
            return true;
        }
        cursor = rest;
    }
    false
}

fn is_phase_filename_tail(rest: &str) -> bool {
    rest.is_empty()
        || rest.chars().next().is_some_and(|c| c.is_ascii_digit())
        || rest.strip_prefix('_').is_some_and(|after| {
            after.is_empty() || after.chars().next().is_some_and(|c| c.is_ascii_digit()) || {
                let mut chars = after.chars();
                let Some(first) = chars.next() else {
                    return false;
                };
                first.is_ascii_alphabetic()
                    && chars
                        .next()
                        .is_none_or(|next| next.is_ascii_digit() || next == '_')
            }
        })
}

fn check_no_tokio_in_lockfile(out: &mut Vec<Violation>) -> Result<()> {
    let path = PathBuf::from("Cargo.lock");
    if !path.exists() {
        return Ok(());
    }
    let content = std::fs::read_to_string(&path).context("reading Cargo.lock")?;
    for (idx, line) in content.lines().enumerate() {
        if line.trim_start().starts_with("name = \"tokio\"") {
            out.push(Violation {
                rule: "conventions:no-tokio",
                path: "Cargo.lock".into(),
                line: Some(idx + 1),
                message: "tokio is denied — this codebase is sync-with-threads (deny.toml)".into(),
            });
        }
    }
    Ok(())
}

fn is_test_path(path_str_norm: &str) -> bool {
    path_str_norm.starts_with("crates/test_support/")
        || path_str_norm.contains("/tests/")
        || path_str_norm.contains("/benches/")
        // A sibling `tests.rs` next to a module file (e.g.
        // `crates/display_map/src/builder/tests.rs`) is the standard
        // Rust convention for hoisted-out unit tests. Treat it as
        // test code so .unwrap() / panic! etc. don't trip the linter.
        || path_str_norm.ends_with("/tests.rs")
}

fn anyhow_allowed(path_str_norm: &str) -> bool {
    ANYHOW_ALLOWED_PREFIXES.iter().any(|p| {
        if p.ends_with('/') {
            path_str_norm.starts_with(p)
        } else {
            path_str_norm == *p
        }
    })
}

fn check_rust_file(path_str_norm: &str, path: &Path, out: &mut Vec<Violation>) -> Result<()> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("reading {path_str_norm}"))?;
    let lines: Vec<&str> = content.lines().collect();

    let len = lines.len();
    if len > FILE_LENGTH_CAP {
        out.push(Violation {
            rule: "conventions:file-length",
            path: path_str_norm.into(),
            line: None,
            message: format!("{len} lines; max is {FILE_LENGTH_CAP}"),
        });
    }

    let is_test_file = is_test_path(path_str_norm);
    let mut depth: i32 = 0;
    let mut test_region_depth: Option<i32> = None;
    let mut pending_cfg_test = false;

    for (idx, raw) in lines.iter().enumerate() {
        let line_no = idx + 1;
        let trimmed = raw.trim_start();
        let code = strip_strings_and_comment(raw);
        let is_attr_line = trimmed.starts_with("#[");
        let is_blank_or_comment =
            trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with("/*");

        if pending_cfg_test {
            if is_blank_or_comment || is_attr_line {
                // hold the arming over attribute / blank lines (`#[cfg(test)]\n#[allow(...)]\nmod tests {`)
            } else if code.contains('{') {
                test_region_depth = Some(depth);
                pending_cfg_test = false;
            } else {
                pending_cfg_test = false;
            }
        } else if trimmed.contains("#[cfg(test)]") {
            if code.contains('{') {
                test_region_depth = Some(depth);
            } else {
                pending_cfg_test = true;
            }
        }

        let in_test_region = test_region_depth.is_some() || is_test_file;

        let comment_part = comment_text(raw);
        check_bare_todo(&comment_part, path_str_norm, line_no, out);

        check_no_async_fn(&code, path_str_norm, line_no, out);

        if !anyhow_allowed(path_str_norm) {
            check_anyhow(&code, path_str_norm, line_no, out);
        }

        if !in_test_region {
            check_no_unwrap_panic(&code, path_str_norm, line_no, out);
            check_no_glob_imports(&code, path_str_norm, line_no, out);
            crate::conventions_b_rules::check_pub_use_only_in_lib_rs(
                &code,
                raw,
                path_str_norm,
                line_no,
                out,
            );
            crate::conventions_b_rules::check_no_use_aliasing(
                &code,
                raw,
                path_str_norm,
                line_no,
                out,
            );
        }

        crate::conventions_identifier_rules::check_phase_prefixed_definition(
            &code,
            path_str_norm,
            line_no,
            out,
        );

        let opens = code.matches('{').count() as i32;
        let closes = code.matches('}').count() as i32;
        depth += opens - closes;
        if let Some(start) = test_region_depth {
            if depth <= start {
                test_region_depth = None;
            }
        }
    }

    Ok(())
}

fn check_bare_todo(comment: &str, path: &str, line: usize, out: &mut Vec<Violation>) {
    let Some(idx) = comment.find("TODO") else {
        return;
    };
    let after = &comment[idx + 4..];
    let bracket_ok = after
        .strip_prefix("(#")
        .map(|s| {
            let digits = s.chars().take_while(|c| c.is_ascii_digit()).count();
            digits > 0 && s[digits..].starts_with(')')
        })
        .unwrap_or(false);
    if !bracket_ok {
        out.push(Violation {
            rule: "conventions:bare-todo",
            path: path.into(),
            line: Some(line),
            message: "bare TODO is forbidden — use `TODO(#123): ...` with a tracking issue".into(),
        });
    }
}

fn check_no_async_fn(code: &str, path: &str, line: usize, out: &mut Vec<Violation>) {
    let Some(idx) = find_word(code, "async") else {
        return;
    };
    let rest = code[idx + "async".len()..].trim_start();
    if find_word(rest, "fn").map(|p| p == 0).unwrap_or(false) {
        out.push(Violation {
            rule: "conventions:no-async-fn",
            path: path.into(),
            line: Some(line),
            message: "async fn is forbidden — this codebase is sync-with-threads".into(),
        });
    }
}

fn check_anyhow(code: &str, path: &str, line: usize, out: &mut Vec<Violation>) {
    if find_word(code, "anyhow").is_some() {
        out.push(Violation {
            rule: "conventions:anyhow-restricted",
            path: path.into(),
            line: Some(line),
            message: "anyhow is allowed only in crates/app/src/main.rs and xtask/".into(),
        });
    }
}

fn check_no_unwrap_panic(code: &str, path: &str, line: usize, out: &mut Vec<Violation>) {
    if code.contains(".unwrap()") {
        out.push(Violation {
            rule: "conventions:no-unwrap-panic",
            path: path.into(),
            line: Some(line),
            message:
                ".unwrap() is forbidden in non-test code — use `?` or `expect(\"invariant: …\")`"
                    .into(),
        });
        return;
    }
    if code.contains("panic!(") {
        out.push(Violation {
            rule: "conventions:no-unwrap-panic",
            path: path.into(),
            line: Some(line),
            message: "panic!(...) is forbidden in non-test code".into(),
        });
        return;
    }
    if code.contains("unreachable!(") {
        out.push(Violation {
            rule: "conventions:no-unwrap-panic",
            path: path.into(),
            line: Some(line),
            message: "unreachable!(...) is forbidden in non-test code".into(),
        });
    }
}

fn check_no_glob_imports(code: &str, path: &str, line: usize, out: &mut Vec<Violation>) {
    let trimmed = code.trim_start();
    if !(trimmed.starts_with("use ") || trimmed.starts_with("pub use ")) {
        return;
    }
    if trimmed.contains("::*;") || trimmed.contains("::*as") {
        out.push(Violation {
            rule: "conventions:no-glob-imports",
            path: path.into(),
            line: Some(line),
            message: "glob imports (`use foo::*;`) are forbidden outside test code".into(),
        });
    }
}

#[cfg(test)]
mod tests;
