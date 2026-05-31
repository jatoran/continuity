//! Phase 17.9 §F — perf history + telemetry.
//!
//! `perf-snapshot` collects p50/p95/p99/p99.9/jitter from every gate
//! into `target/perf/snapshot-<sha>.json`. `perf-history-append` appends
//! the latest snapshot to the tracked `.perf/history.jsonl` (idempotent
//! on `(sha, host_id)`). `perf-report` prints / charts trend.
//! `perf-compare --baseline <sha>` exits non-zero on regression.
//!
//! The collection mechanism: every perf gate's `assert_within_budget`
//! writes a JSON line per `label` into `$CONTINUITY_PERF_LOG_DIR` when
//! that env var is set. `snapshot()` sets it to a per-run temp dir,
//! runs every gate, then aggregates the per-label files into one
//! snapshot keyed by `(crate_name, test_target)` plus the gate label.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct GateStats {
    pub label: String,
    pub p50_us: u128,
    pub p95_us: u128,
    pub p99_us: u128,
    pub p99_9_us: u128,
    pub max_us: u128,
    pub p99_budget_us: u128,
    pub jitter: f64,
    pub sample_count: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PerfSnapshot {
    pub git_sha: String,
    pub timestamp_unix: u64,
    pub host_id: String,
    pub rustc_version: String,
    pub samples: BTreeMap<String, GateStats>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CheckAllOutcome {
    pub pass: bool,
    pub failed: Vec<String>,
    pub snapshot_path: String,
}

pub fn snapshot(args: &[String]) -> Result<()> {
    let path = snapshot_with_path(args)?;
    eprintln!("wrote {}", path.display());
    Ok(())
}

pub fn snapshot_with_path(_args: &[String]) -> Result<PathBuf> {
    let workspace = workspace_root();
    let log_dir = workspace.join("target").join("perf").join("gates");
    if log_dir.exists() {
        fs::remove_dir_all(&log_dir).ok();
    }
    fs::create_dir_all(&log_dir)?;

    let gates = [
        ("continuity-buffer", "perf_gates"),
        ("continuity-core", "perf_gates"),
        ("continuity-decorate", "perf_gates"),
        ("continuity-display-map", "perf_gates"),
        ("continuity-render", "perf_gates"),
        ("continuity-persist", "perf_gates"),
        ("continuity-search", "perf_gates"),
        ("continuity-ui", "perf_gates"),
        ("continuity-test-support", "perf_gates_memory_empty"),
        ("continuity-test-support", "perf_gates_memory_50"),
        ("continuity-test-support", "perf_gates_memory_200"),
    ];
    let mut failures: Vec<String> = Vec::new();
    for (krate, target) in gates {
        eprintln!("---- perf gate: {krate} :: {target} ----");
        let status = Command::new(env!("CARGO"))
            .args([
                "test",
                "--release",
                "-p",
                krate,
                "--test",
                target,
                "--",
                "--ignored",
                "--test-threads=1",
                "--nocapture",
            ])
            .env("CONTINUITY_PERF_LOG_DIR", &log_dir)
            .status()?;
        if !status.success() {
            failures.push(format!("{krate}::{target}"));
        }
    }

    let mut samples = BTreeMap::new();
    if log_dir.exists() {
        for entry in fs::read_dir(&log_dir)? {
            let entry = entry?;
            let bytes = fs::read_to_string(entry.path())?;
            let stats: GateStats = serde_json::from_str(bytes.trim())
                .map_err(|e| anyhow!("parse {}: {e}", entry.path().display()))?;
            samples.insert(stats.label.clone(), stats);
        }
    }

    let snap = PerfSnapshot {
        git_sha: git_sha(),
        timestamp_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        host_id: host_id(),
        rustc_version: rustc_version(),
        samples,
    };

    let out_dir = workspace.join("target").join("perf");
    fs::create_dir_all(&out_dir)?;
    let out_path = out_dir.join(format!("snapshot-{}.json", snap.git_sha));
    fs::write(&out_path, serde_json::to_string_pretty(&snap)?)?;

    if !failures.is_empty() {
        bail!("{} gate(s) failed: {}", failures.len(), failures.join(", "));
    }
    Ok(out_path)
}

pub fn history_append(_args: &[String]) -> Result<()> {
    let workspace = workspace_root();
    let snap_path = latest_snapshot()?;
    let bytes = fs::read_to_string(&snap_path)?;
    let snap: PerfSnapshot = serde_json::from_str(&bytes)?;

    let history_dir = workspace.join(".perf");
    fs::create_dir_all(&history_dir)?;
    let history_path = history_dir.join("history.jsonl");

    let mut existing: Vec<PerfSnapshot> = if history_path.exists() {
        fs::read_to_string(&history_path)?
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()?
    } else {
        Vec::new()
    };
    existing.retain(|s| !(s.git_sha == snap.git_sha && s.host_id == snap.host_id));
    existing.push(snap);

    let mut out = String::new();
    for s in &existing {
        out.push_str(&serde_json::to_string(s)?);
        out.push('\n');
    }
    fs::write(&history_path, out)?;
    eprintln!(
        "appended {} → {}",
        snap_path.display(),
        history_path.display()
    );
    Ok(())
}

pub fn report(args: &[String]) -> Result<()> {
    let mut last: Option<usize> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--last" => {
                let n = args
                    .get(i + 1)
                    .ok_or_else(|| anyhow!("--last requires N"))?
                    .parse()?;
                last = Some(n);
                i += 2;
            }
            other => bail!("perf-report: unknown arg `{other}`"),
        }
    }
    let history_path = workspace_root().join(".perf").join("history.jsonl");
    if !history_path.exists() {
        bail!("no .perf/history.jsonl — run `perf-history-append` first");
    }
    let snaps: Vec<PerfSnapshot> = fs::read_to_string(&history_path)?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    let take_n = last.unwrap_or(snaps.len());
    let start = snaps.len().saturating_sub(take_n);
    let view = &snaps[start..];

    let mut all_gates: Vec<&str> = view
        .iter()
        .flat_map(|s| s.samples.keys().map(String::as_str))
        .collect();
    all_gates.sort();
    all_gates.dedup();

    println!(
        "perf history ({} snapshots, {} gates)",
        view.len(),
        all_gates.len()
    );
    for gate in &all_gates {
        println!();
        println!("  {}", gate);
        println!(
            "  {:<10}  {:>10}  {:>10}  {:>10}",
            "sha", "p99(µs)", "p99.9(µs)", "jitter"
        );
        for snap in view {
            if let Some(g) = snap.samples.get(*gate) {
                println!(
                    "  {:<10}  {:>10}  {:>10}  {:>10.2}",
                    short(&snap.git_sha),
                    g.p99_us,
                    g.p99_9_us,
                    g.jitter,
                );
            }
        }
    }
    Ok(())
}

pub fn compare(args: &[String]) -> Result<()> {
    let mut baseline: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--baseline" => {
                baseline = Some(
                    args.get(i + 1)
                        .ok_or_else(|| anyhow!("--baseline requires <sha>"))?
                        .clone(),
                );
                i += 2;
            }
            other => bail!("perf-compare: unknown arg `{other}`"),
        }
    }
    let baseline = baseline.ok_or_else(|| anyhow!("--baseline <sha> required"))?;
    let history_path = workspace_root().join(".perf").join("history.jsonl");
    if !history_path.exists() {
        bail!("no .perf/history.jsonl");
    }
    let snaps: Vec<PerfSnapshot> = fs::read_to_string(&history_path)?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    let base = snaps
        .iter()
        .find(|s| s.git_sha.starts_with(&baseline))
        .ok_or_else(|| anyhow!("baseline sha {} not found in history", baseline))?;
    let current_path = latest_snapshot()?;
    let current: PerfSnapshot = serde_json::from_str(&fs::read_to_string(&current_path)?)?;

    let mut regressions: Vec<String> = Vec::new();
    for (label, cur) in &current.samples {
        let Some(b) = base.samples.get(label) else {
            continue;
        };
        let p99_growth = ratio(cur.p99_us, b.p99_us);
        let p99_9_growth = ratio(cur.p99_9_us, b.p99_9_us);
        if p99_growth > 1.10 {
            regressions.push(format!(
                "{label}: p99 {} → {} µs ({:.0}%)",
                b.p99_us,
                cur.p99_us,
                (p99_growth - 1.0) * 100.0
            ));
        }
        if p99_9_growth > 1.20 {
            regressions.push(format!(
                "{label}: p99.9 {} → {} µs ({:.0}%)",
                b.p99_9_us,
                cur.p99_9_us,
                (p99_9_growth - 1.0) * 100.0
            ));
        }
    }
    if regressions.is_empty() {
        eprintln!("perf-compare: no regressions vs {}", short(&base.git_sha));
        return Ok(());
    }
    for r in &regressions {
        eprintln!("REGRESSION  {r}");
    }
    bail!("{} regression(s)", regressions.len());
}

fn ratio(now: u128, then: u128) -> f64 {
    if then == 0 {
        return 1.0;
    }
    now as f64 / then as f64
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn git_sha() -> String {
    Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn host_id() -> String {
    std::env::var("CONTINUITY_HOST_ID")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

fn rustc_version() -> String {
    Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn latest_snapshot() -> Result<PathBuf> {
    let dir = workspace_root().join("target").join("perf");
    let sha = git_sha();
    let by_sha = dir.join(format!("snapshot-{sha}.json"));
    if by_sha.exists() {
        return Ok(by_sha);
    }
    let mut latest: Option<(SystemTime, PathBuf)> = None;
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("snapshot-") && n.ends_with(".json"))
                .unwrap_or(false)
            {
                let mtime = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(UNIX_EPOCH);
                if latest.as_ref().map(|(t, _)| mtime > *t).unwrap_or(true) {
                    latest = Some((mtime, p));
                }
            }
        }
    }
    latest
        .map(|(_, p)| p)
        .ok_or_else(|| anyhow!("no snapshot under {}", dir.display()))
}
