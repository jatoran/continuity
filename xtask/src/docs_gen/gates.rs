//! Gate and task generated docs.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::docs_gen::{escape_md_cell, new_doc, normalize_path};

pub(crate) fn write_gates(workspace: &Path) -> Result<String> {
    let tasks = parse_xtask_tasks(&workspace.join("xtask/src/main.rs"))?;
    let hooks = parse_hook_commands(workspace)?;
    let ci = parse_ci_commands(workspace)?;
    let perf_gates = parse_perf_gates(&workspace.join("xtask/src/bench.rs"))?;
    let mut out = new_doc("Gates");
    out.push_str("Generated from `xtask/src/main.rs`, `.githooks/*`, `.github/workflows/*`, and `xtask/src/bench.rs`.\n\n");
    write_xtask_table(&mut out, &tasks, &hooks, &ci);
    write_hook_table(&mut out, &hooks);
    write_ci_table(&mut out, &ci);
    write_perf_table(&mut out, &perf_gates);
    write_drift_warnings(&mut out, &tasks, &hooks, &ci);
    Ok(out)
}

#[derive(Debug)]
struct TaskInfo {
    name: String,
    action: String,
}

#[derive(Debug)]
struct CommandUse {
    surface: String,
    command: String,
}

#[derive(Debug)]
struct PerfGate {
    crate_name: String,
    test_target: String,
    fast: bool,
}

fn parse_xtask_tasks(path: &Path) -> Result<Vec<TaskInfo>> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let all_lines: Vec<&str> = text.lines().collect();
    let lines = task_match_lines(&all_lines);
    let mut tasks = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let Some((left, right)) = line.split_once("=>") else {
            continue;
        };
        let commands = task_names_for_arm(left);
        if commands.is_empty() {
            continue;
        }
        let arm = task_arm_lines(&lines, idx);
        let action = summarize_action(right, &arm);
        for name in commands {
            tasks.push(TaskInfo {
                name,
                action: action.clone(),
            });
        }
    }
    tasks.sort_by(|a, b| a.name.cmp(&b.name));
    tasks.dedup_by(|a, b| a.name == b.name);
    Ok(tasks)
}

fn task_match_lines<'a>(lines: &'a [&'a str]) -> Vec<&'a str> {
    let Some(start) = lines
        .iter()
        .position(|line| line.contains("match task.as_str()"))
    else {
        return Vec::new();
    };
    let end = lines[start + 1..]
        .iter()
        .position(|line| line.trim() == "};")
        .map_or(lines.len(), |idx| start + 1 + idx);
    lines[start + 1..end].to_vec()
}

fn task_names_for_arm(left: &str) -> Vec<String> {
    if !left.trim_start().starts_with('"') {
        return Vec::new();
    }
    quoted_strings(left)
        .into_iter()
        .filter(|name| !name.starts_with('-'))
        .collect()
}

fn task_arm_lines<'a>(lines: &[&'a str], start: usize) -> Vec<&'a str> {
    let end = lines[start + 1..]
        .iter()
        .position(|line| {
            line.split_once("=>")
                .is_some_and(|(left, _)| !task_names_for_arm(left).is_empty())
        })
        .map_or(lines.len(), |idx| start + 1 + idx);
    lines[start..end].to_vec()
}

fn parse_hook_commands(workspace: &Path) -> Result<Vec<CommandUse>> {
    let dir = workspace.join(".githooks");
    let mut uses = Vec::new();
    if !dir.exists() {
        return Ok(uses);
    }
    for path in sorted_files(&dir)? {
        let surface = normalize_path(path.strip_prefix(workspace).unwrap_or(&path));
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        for line in active_lines(&text) {
            if let Some(command) = parse_cargo_xtask(line) {
                uses.push(CommandUse {
                    surface: surface.clone(),
                    command,
                });
            }
        }
    }
    uses.sort_by(|a, b| {
        a.surface
            .cmp(&b.surface)
            .then_with(|| a.command.cmp(&b.command))
    });
    Ok(uses)
}

fn parse_ci_commands(workspace: &Path) -> Result<Vec<CommandUse>> {
    let dir = workspace.join(".github/workflows");
    let mut uses = Vec::new();
    if !dir.exists() {
        return Ok(uses);
    }
    for path in sorted_files(&dir)? {
        let surface = normalize_path(path.strip_prefix(workspace).unwrap_or(&path));
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        for line in active_lines(&text) {
            if let Some(command) = parse_cargo_xtask(line) {
                uses.push(CommandUse {
                    surface: surface.clone(),
                    command,
                });
            }
        }
    }
    uses.sort_by(|a, b| {
        a.surface
            .cmp(&b.surface)
            .then_with(|| a.command.cmp(&b.command))
    });
    Ok(uses)
}

fn parse_perf_gates(path: &Path) -> Result<Vec<PerfGate>> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut gates = Vec::new();
    let mut crate_name = None;
    let mut test_target = None;
    let mut fast = None;
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(value) = field_string(line, "crate_name") {
            crate_name = Some(value);
        } else if let Some(value) = field_string(line, "test_target") {
            test_target = Some(value);
        } else if let Some(value) = field_bool(line, "fast") {
            fast = Some(value);
        }
        if line == "}," {
            if let (Some(crate_name), Some(test_target), Some(fast)) =
                (crate_name.take(), test_target.take(), fast.take())
            {
                gates.push(PerfGate {
                    crate_name,
                    test_target,
                    fast,
                });
            }
        }
    }
    Ok(gates)
}

fn write_xtask_table(
    out: &mut String,
    tasks: &[TaskInfo],
    hooks: &[CommandUse],
    ci: &[CommandUse],
) {
    out.push_str("## xtask Commands\n\n");
    out.push_str("| Command | Implementation | Hook surfaces | CI surfaces | Cost tier |\n");
    out.push_str("|---|---|---|---|---|\n");
    for task in tasks {
        out.push_str(&format!(
            "| `cargo xtask {}` | `{}` | {} | {} | {} |\n",
            task.name,
            escape_md_cell(&task.action),
            surfaces_for(hooks, &task.name),
            surfaces_for(ci, &task.name),
            cost_tier(&task.name, hooks)
        ));
    }
    out.push('\n');
}

fn write_hook_table(out: &mut String, hooks: &[CommandUse]) {
    out.push_str("## Hook Membership\n\n");
    write_use_table(out, hooks);
}

fn write_ci_table(out: &mut String, ci: &[CommandUse]) {
    out.push_str("## CI Membership\n\n");
    write_use_table(out, ci);
}

fn write_perf_table(out: &mut String, gates: &[PerfGate]) {
    out.push_str("## Perf Gates\n\n");
    out.push_str("| Crate | Test target | Fast subset | Command |\n");
    out.push_str("|---|---|---|---|\n");
    for gate in gates {
        out.push_str(&format!(
            "| `{}` | `{}` | {} | `cargo test --release -p {} --test {} -- --ignored --test-threads=1 --nocapture` |\n",
            gate.crate_name,
            gate.test_target,
            if gate.fast { "yes" } else { "full only" },
            gate.crate_name,
            gate.test_target
        ));
    }
    out.push_str("| `continuity-app` | release binary size | yes | `cargo build --release -p continuity-app` |\n\n");
}

fn write_drift_warnings(
    out: &mut String,
    tasks: &[TaskInfo],
    hooks: &[CommandUse],
    ci: &[CommandUse],
) {
    let known = tasks
        .iter()
        .map(|task| task.name.as_str())
        .collect::<BTreeSet<_>>();
    let mut missing = hooks
        .iter()
        .chain(ci.iter())
        .filter(|entry| !known.contains(entry.command.as_str()))
        .collect::<Vec<_>>();
    missing.sort_by(|a, b| {
        a.surface
            .cmp(&b.surface)
            .then_with(|| a.command.cmp(&b.command))
    });
    out.push_str("## Drift Warnings\n\n");
    if missing.is_empty() {
        out.push_str("None.\n");
        return;
    }
    for entry in missing {
        out.push_str(&format!(
            "- `{}` references unknown `cargo xtask {}`.\n",
            entry.surface, entry.command
        ));
    }
}

fn write_use_table(out: &mut String, uses: &[CommandUse]) {
    if uses.is_empty() {
        out.push_str("None.\n\n");
        return;
    }
    out.push_str("| Surface | Command |\n");
    out.push_str("|---|---|\n");
    for entry in uses {
        out.push_str(&format!(
            "| `{}` | `cargo xtask {}` |\n",
            entry.surface, entry.command
        ));
    }
    out.push('\n');
}

fn sorted_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = fs::read_dir(dir)
        .with_context(|| format!("listing {}", dir.display()))?
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("reading {}", dir.display()))?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

fn active_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
}

fn parse_cargo_xtask(line: &str) -> Option<String> {
    let idx = line.find("cargo xtask ")?;
    let rest = &line[idx + "cargo xtask ".len()..];
    let command = rest
        .split_whitespace()
        .next()?
        .trim_matches(|c: char| c == '"' || c == '\'' || c == '`');
    (!command.is_empty()).then(|| command.to_string())
}

fn quoted_strings(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(start) = rest.find('"') {
        let after_start = &rest[start + 1..];
        let Some(end) = after_start.find('"') else {
            break;
        };
        out.push(after_start[..end].to_string());
        rest = &after_start[end + 1..];
    }
    out
}

fn summarize_action(first: &str, lines: &[&str]) -> String {
    if let Some(args) = cargo_args(lines) {
        return format!("cargo {}", args.join(" "));
    }
    let joined = lines.join(" ");
    if joined.contains("print_help()") {
        return "print_help()".into();
    }
    if joined.contains("conventions::check_commit_msg") {
        return "conventions::check_commit_msg(<file>)".into();
    }
    let mut snippet = first.trim().trim_end_matches(',').to_string();
    if snippet == "{" {
        snippet = "block".into();
    }
    snippet
}

fn cargo_args(lines: &[&str]) -> Option<Vec<String>> {
    let joined = lines.iter().take(12).copied().collect::<Vec<_>>().join(" ");
    let start = joined.find("run_cargo(&[")?;
    let after = &joined[start + "run_cargo(&[".len()..];
    let end = after.find("]")?;
    Some(quoted_strings(&after[..end]))
}

fn field_string(line: &str, name: &str) -> Option<String> {
    let prefix = format!("{name}: ");
    let rest = line.strip_prefix(&prefix)?;
    quoted_strings(rest).into_iter().next()
}

fn field_bool(line: &str, name: &str) -> Option<bool> {
    let prefix = format!("{name}: ");
    let rest = line.strip_prefix(&prefix)?;
    if rest.starts_with("true") {
        Some(true)
    } else if rest.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

fn surfaces_for(uses: &[CommandUse], command: &str) -> String {
    let surfaces = uses
        .iter()
        .filter(|entry| entry.command == command)
        .map(|entry| format!("`{}`", entry.surface))
        .collect::<Vec<_>>();
    surfaces.join("<br>")
}

fn cost_tier(command: &str, hooks: &[CommandUse]) -> &'static str {
    if hooks
        .iter()
        .any(|entry| entry.command == command && entry.surface.ends_with("pre-commit"))
    {
        "fast"
    } else if hooks.iter().any(|entry| entry.command == command) {
        "fat"
    } else if command.contains("bench") || command.contains("check-all") || command.contains("perf")
    {
        "release/heavy"
    } else {
        "manual"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_active_xtask_line() {
        assert_eq!(
            parse_cargo_xtask("run: cargo xtask docs-check"),
            Some("docs-check".into())
        );
        assert_eq!(parse_cargo_xtask("# cargo xtask ci"), Some("ci".into()));
    }

    #[test]
    fn quoted_string_parser_extracts_match_arm_aliases() {
        assert_eq!(quoted_strings("\"help\" | \"-h\""), vec!["help", "-h"]);
    }

    #[test]
    fn xtask_parser_keeps_actions_inside_each_task_arm() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("main.rs");
        fs::write(
            &path,
            r#"
fn main() {
    let result = match task.as_str() {
        "help" | "-h" => {
            print_help();
            Ok(())
        }
        "check" => run_cargo(&["check", "--workspace"]),
        "check-commit-msg" => match rest.first() {
            Some(p) => conventions::check_commit_msg(Path::new(p)),
            None => Err(anyhow!("usage")),
        },
        other => Err(anyhow!("unknown")),
    };
}
"#,
        )
        .expect("write fixture");
        let tasks = parse_xtask_tasks(&path).expect("parse tasks");
        let help = tasks
            .iter()
            .find(|task| task.name == "help")
            .expect("help task");
        let check = tasks
            .iter()
            .find(|task| task.name == "check")
            .expect("check task");
        let commit = tasks
            .iter()
            .find(|task| task.name == "check-commit-msg")
            .expect("commit task");

        assert_eq!(help.action, "print_help()");
        assert_eq!(check.action, "cargo check --workspace");
        assert_eq!(commit.action, "conventions::check_commit_msg(<file>)");
    }
}
