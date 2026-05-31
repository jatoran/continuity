//! Build / test / bench / release runner for the continuity workspace.
//!
//! Invoke via `cargo xtask <task>`. Aliased in `.cargo/config.toml`.

mod analyze_trace;
mod analyze_trace_memory_sections;
mod analyze_trace_sections;
mod bench;
mod conventions;
mod conventions_b_rules;
mod conventions_identifier_rules;
mod docs_gen;
mod installer;
mod perf_history;
mod release;
mod scan;
mod tutorial_gen;

use std::env;
use std::path::Path;
use std::process::{Command, ExitCode};

use anyhow::{anyhow, bail, Result};

fn main() -> ExitCode {
    let task = env::args().nth(1).unwrap_or_else(|| "help".to_string());
    let rest: Vec<String> = env::args().skip(2).collect();

    let result = match task.as_str() {
        "help" | "-h" | "--help" => {
            print_help();
            Ok(())
        }
        "check" => run_cargo(&["check", "--workspace", "--all-targets"]),
        "test" => run_cargo(&["test", "--workspace"]),
        "fmt" => run_cargo(&["fmt", "--all", "--", "--check"]),
        "clippy" => run_cargo(&[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ]),
        "ci" => run_ci(),
        "bench" => bench::run(false),
        "bench-fast" => bench::run(true),
        "conventions" => conventions::run(),
        "docs" => docs_gen::run_write(),
        "docs-check" => docs_gen::run_check(),
        "install-hooks" => conventions::install_hooks(),
        "check-commit-msg" => match rest.first() {
            Some(p) => conventions::check_commit_msg(Path::new(p)),
            None => Err(anyhow!("usage: cargo xtask check-commit-msg <file>")),
        },
        "snapshot-canary" => run_snapshot_canary(false),
        "snapshot-update" => run_snapshot_canary(true),
        "e2e-smoke" => run_e2e_smoke(),
        "test-all" => run_test_all(),
        "check-all" => run_check_all(),
        "agent-check" => run_agent_check(),
        "perf-snapshot" => perf_history::snapshot(&rest),
        "perf-history-append" => perf_history::history_append(&rest),
        "perf-report" => perf_history::report(&rest),
        "perf-compare" => perf_history::compare(&rest),
        "package" => release::package(),
        "installer" => installer::installer(),
        "sign" => release::sign(),
        "release" => release::release(&rest),
        "gen-tutorial" => tutorial_gen::run(),
        "analyze-trace" => analyze_trace::run(&rest),
        other => Err(anyhow!("unknown xtask `{other}` — try `help`")),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    println!("xtask — workspace task runner");
    println!();
    println!("USAGE: cargo xtask <task>");
    println!();
    println!("TASKS:");
    println!("  help              show this message");
    println!("  check             cargo check --workspace --all-targets");
    println!("  test              cargo test --workspace");
    println!("  fmt               cargo fmt --all -- --check");
    println!("  clippy            cargo clippy --workspace --all-targets -- -D warnings");
    println!("  ci                fast tier — fmt, clippy, check, test (no conventions, no perf, no e2e)");
    println!("  bench             run every Phase 17 perf gate (full set, ~minutes)");
    println!("  bench-fast        run the CI-friendly subset of Phase 17 perf gates");
    println!("  conventions       repo convention checks (file length, no-mod-rs,");
    println!("                    no unwrap/panic, anyhow scope, glob imports,");
    println!("                    bare TODO, async fn, tokio in lockfile)");
    println!("  docs              regenerate .docs/generated/");
    println!("  docs-check        regenerate generated docs in memory and fail on drift");
    println!("  install-hooks     point git core.hooksPath at the checked-in .githooks/");
    println!("  check-commit-msg <file>");
    println!("                    validate a Conventional Commits message file");
    println!("                    (used by .githooks/commit-msg)");
    println!();
    println!("Phase 17.9 testing + perf:");
    println!("  snapshot-canary   run the §D pixel canary in compare mode");
    println!("  snapshot-update   regenerate pixel-canary golden hashes");
    println!("  e2e-smoke         run the cheapest §C e2e tests (smoke + pane split)");
    println!("  test-all          ci + every E2E test + pixel canary (correctness pass)");
    println!("  check-all         test-all + bench + perf-snapshot (\"is this shippable?\")");
    println!("  agent-check       docs-check + check-all with structured JSON to stdout");
    println!("  perf-snapshot     run every gate; write target/perf/snapshot-<sha>.json");
    println!("  perf-history-append");
    println!("                    append the latest snapshot to .perf/history.jsonl");
    println!("  perf-report [--last N] [--svg <path>]");
    println!("                    print or chart the per-gate p99/p99.9/jitter trend");
    println!("  perf-compare --baseline <sha>");
    println!(
        "                    compare current snapshot against a baseline; non-zero on regression"
    );
    println!();
    println!("Phase 18 release engineering:");
    println!("  package           build release exe and continuity-portable.zip");
    println!("  installer         build release exe, portable zip, and continuity-setup.msi");
    println!("  sign              sign the release exe using CONTINUITY_SIGN_CERT/PASS");
    println!("  release           build, sign, and assemble release artifacts including MSI");
    println!("                    pass --skip-sign for local unsigned artifact smoke tests");
    println!();
    println!("Tutorial:");
    println!("  gen-tutorial      regenerate crates/command/assets/tutorial.md from");
    println!("                    .docs/design/features/*.md + crates/keymap/assets/default.toml");
    println!();
    println!("Trace analysis:");
    println!("  analyze-trace <path> [--label <sub>] [--edit-seq <N>]");
    println!("                    parse a CONTINUITY_UI_TRACE TSV, emit a markdown report");
    println!("                    (per-label percentiles, stalls, top-K slowest, cold paint,");
    println!(
        "                    edit-seq cross-thread stitch, worker queue depth, process state)"
    );
}

fn run_snapshot_canary(update: bool) -> Result<()> {
    // F5 Pass 2 — the inline-image canary lives in its own
    // integration-test binary so the text-only fixtures stay focused.
    // Both binaries share the same fixture directory + golden-hash
    // convention; we run them sequentially.
    for test_bin in ["pixel_canary", "pixel_canary_inline_image"] {
        let mut cmd = Command::new(env!("CARGO"));
        cmd.args([
            "test",
            "-p",
            "continuity-render",
            "--release",
            "--test",
            test_bin,
        ]);
        if update {
            cmd.env("CONTINUITY_PIXEL_CANARY_UPDATE", "1");
        }
        let status = cmd.status()?;
        if !status.success() {
            bail!(
                "pixel canary `{}` {} failed (exit {:?})",
                test_bin,
                if update { "update" } else { "compare" },
                status.code()
            );
        }
    }
    Ok(())
}

fn run_e2e_smoke() -> Result<()> {
    for test in ["e2e_smoke", "e2e_pane_split"] {
        run_cargo(&["test", "-p", "continuity-ui", "--test", test])?;
    }
    Ok(())
}

fn run_test_all() -> Result<()> {
    run_ci()?;
    for test in [
        "e2e_smoke",
        "e2e_pane_split",
        "e2e_multi_window",
        "e2e_settings_live_reload",
    ] {
        run_cargo(&["test", "-p", "continuity-ui", "--test", test])?;
    }
    run_snapshot_canary(false)?;
    Ok(())
}

fn run_check_all() -> Result<()> {
    run_test_all()?;
    bench::run(false)?;
    perf_history::snapshot(&[])?;
    Ok(())
}

fn run_agent_check() -> Result<()> {
    let outcome = (|| -> Result<perf_history::CheckAllOutcome> {
        docs_gen::check()?;
        run_test_all()?;
        bench::run(false)?;
        let snapshot_path = perf_history::snapshot_with_path(&[])?;
        Ok(perf_history::CheckAllOutcome {
            pass: true,
            failed: vec![],
            snapshot_path: snapshot_path.display().to_string(),
        })
    })();
    let result = outcome.unwrap_or_else(|e| perf_history::CheckAllOutcome {
        pass: false,
        failed: vec![e.to_string()],
        snapshot_path: String::new(),
    });
    let json = serde_json::to_string(&result)?;
    println!("{json}");
    eprintln!(
        "agent-check: {} ({} failures)",
        if result.pass { "PASS" } else { "FAIL" },
        result.failed.len()
    );
    if !result.pass {
        bail!("agent-check failed");
    }
    Ok(())
}

fn run_cargo(args: &[&str]) -> Result<()> {
    let status = Command::new(env!("CARGO")).args(args).status()?;
    if !status.success() {
        bail!("cargo {} failed (exit {:?})", args.join(" "), status.code());
    }
    Ok(())
}

fn run_ci() -> Result<()> {
    // Phase 17.9 §H1: pre-commit fast tier — fmt + clippy + check + unit
    // tests only. Conventions, perf gates, E2E, and the pixel canary all
    // live in heavier tiers (`xtask conventions`, `bench-fast` /
    // `bench`, `e2e-smoke`, `snapshot-canary`) so a green `ci` clears in
    // well under 90 s on a warm cache.
    run_cargo(&["fmt", "--all", "--", "--check"])?;
    run_cargo(&[
        "clippy",
        "--workspace",
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ])?;
    run_cargo(&["check", "--workspace", "--all-targets"])?;
    run_cargo(&["test", "--workspace"])?;
    Ok(())
}
