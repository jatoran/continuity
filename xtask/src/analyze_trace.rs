//! `cargo xtask analyze-trace <path>` — summarize a TSV trace.
//!
//! Reads a `CONTINUITY_UI_TRACE` TSV file and emits a markdown report
//! tuned for LLM/agent consumption. No external dependencies, no
//! visualization — every section is a human- and parser-friendly
//! markdown table or numbered list. The intended workflow is:
//!
//! ```text
//! $ cargo xtask analyze-trace perf-snapshots/manual-lag_<latest>.tsv
//! ```
//!
//! Filters:
//! - `--label <substring>`: only include events whose label contains
//!   the substring (substring match, case-sensitive).
//! - `--edit-seq <N>`: only the cross-thread timeline for one edit.
//!
//! Sections emitted (in order):
//! 1. Trace metadata (from `trace_open`).
//! 2. Per-label percentile summary (from the last `running_summary`
//!    line per label, when present).
//! 3. Stall + stall100 listing plus captured stall stacks.
//! 4. Top-K slowest events overall.
//! 5. Worst-paint rows with render stats and draw stages.
//! 6. Cold-paint signature (`paint:frame_display:cold_build` etc).
//! 7. Row-index cache miss reasons.
//! 8. Edit-seq cross-thread stitching (UI / core / persist lines for
//!    the same `edit_seq=N`).
//! 9. Projection-worker queue depth highs.
//! 10. memory/process_state low/high watermarks including power state.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

use anyhow::{bail, Result};

#[derive(Debug, Clone)]
pub(crate) struct TraceRow {
    pub(crate) ms_since_start: u64,
    pub(crate) kind: String,
    pub(crate) label: String,
    pub(crate) duration_us: u64,
    pub(crate) details: String,
}

/// Pull a `key=value` field out of a space-separated details string.
pub(crate) fn field<'a>(details: &'a str, key: &str) -> Option<&'a str> {
    for token in details.split_whitespace() {
        if let Some((k, v)) = token.split_once('=') {
            if k == key {
                return Some(v);
            }
        }
    }
    None
}

fn parse_row(line: &str) -> Option<TraceRow> {
    let mut cols = line.splitn(5, '\t');
    let ms = cols.next()?.parse::<u64>().ok()?;
    let kind = cols.next()?.to_string();
    let label = cols.next()?.to_string();
    let dur = cols.next()?.parse::<u64>().ok()?;
    let details = cols.next().unwrap_or("").to_string();
    Some(TraceRow {
        ms_since_start: ms,
        kind,
        label,
        duration_us: dur,
        details,
    })
}

#[derive(Default)]
pub(crate) struct Filters {
    pub(crate) label_substring: Option<String>,
    pub(crate) edit_seq: Option<u64>,
}

pub fn run(args: &[String]) -> Result<()> {
    let mut path: Option<&str> = None;
    let mut filters = Filters::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--label" => {
                i += 1;
                let v = args
                    .get(i)
                    .ok_or_else(|| anyhow::anyhow!("--label needs a value"))?;
                filters.label_substring = Some(v.clone());
            }
            "--edit-seq" => {
                i += 1;
                let v = args
                    .get(i)
                    .ok_or_else(|| anyhow::anyhow!("--edit-seq needs a value"))?;
                filters.edit_seq = Some(v.parse()?);
            }
            other if other.starts_with("--") => bail!("unknown flag: {other}"),
            other => {
                if path.is_some() {
                    bail!("only one trace path argument is supported");
                }
                path = Some(other);
            }
        }
        i += 1;
    }
    let path =
        path.ok_or_else(|| anyhow::anyhow!("usage: cargo xtask analyze-trace <trace.tsv>"))?;
    let report = build_report(Path::new(path), &filters)?;
    println!("{report}");
    Ok(())
}

fn build_report(path: &Path, filters: &Filters) -> Result<String> {
    let raw = fs::read_to_string(path)?;
    let rows: Vec<TraceRow> = raw.lines().filter_map(parse_row).collect();
    let filtered: Vec<&TraceRow> = rows
        .iter()
        .filter(|r| match (&filters.label_substring, filters.edit_seq) {
            (None, None) => true,
            (Some(s), None) => r.label.contains(s),
            (None, Some(seq)) => {
                field(&r.details, "edit_seq").and_then(|v| v.parse::<u64>().ok()) == Some(seq)
            }
            (Some(s), Some(seq)) => {
                r.label.contains(s)
                    && field(&r.details, "edit_seq").and_then(|v| v.parse::<u64>().ok())
                        == Some(seq)
            }
        })
        .collect();

    let mut out = String::new();
    out.push_str(&format!("# Trace analysis: {}\n\n", path.display()));
    out.push_str(&format!(
        "Rows read: {} (filtered: {})\n\n",
        rows.len(),
        filtered.len()
    ));

    out.push_str(&metadata_section(&rows));
    out.push_str(&running_summary_section(&rows));
    out.push_str(&stalls_section(&filtered));
    out.push_str(&crate::analyze_trace_sections::stall_stacks_section(
        &filtered,
    ));
    out.push_str(&top_slowest_section(&filtered, 15));
    out.push_str(&crate::analyze_trace_sections::worst_paints_section(&rows));
    out.push_str(&cold_paint_section(&filtered));
    out.push_str(&crate::analyze_trace_sections::row_index_cache_section(
        &rows,
    ));
    out.push_str(&edit_seq_section(&rows, filters));
    out.push_str(&crate::analyze_trace_sections::edit_detail_auto_section(
        &rows, filters,
    ));
    out.push_str(&worker_queue_section(&rows));
    out.push_str(&crate::analyze_trace_sections::command_timeline_section(
        &rows,
    ));
    out.push_str(&crate::analyze_trace_sections::lifecycle_section(&rows));
    out.push_str(&crate::analyze_trace_sections::closed_history_section(
        &rows,
    ));
    out.push_str(&crate::analyze_trace_memory_sections::memory_breakdown_section(&rows));
    out.push_str(&crate::analyze_trace_memory_sections::buffer_focus_events_section(&rows));
    out.push_str(&crate::analyze_trace_memory_sections::decoration_cache_top_section(&rows));
    out.push_str(&process_state_section(&rows));

    Ok(out)
}

fn metadata_section(rows: &[TraceRow]) -> String {
    let mut s = String::from("## Metadata\n\n");
    if let Some(open) = rows.iter().find(|r| r.label == "trace_open") {
        for field in open.details.split_whitespace() {
            s.push_str(&format!("- `{field}`\n"));
        }
    } else {
        s.push_str("(no `trace_open` line found)\n");
    }
    s.push('\n');
    s
}

fn running_summary_section(rows: &[TraceRow]) -> String {
    // For each label, find the LAST running_summary line that
    // mentions it. running_summary details look like
    // `label=foo n=… p50_us=… p95_us=… p99_us=… max_us=… sum_us=…
    // stalls=… stalls100=… b_lt_100us=… …`.
    let mut latest: HashMap<String, &TraceRow> = HashMap::new();
    for r in rows {
        if r.label == "running_summary" {
            if let Some(lbl) = field(&r.details, "label") {
                latest.insert(lbl.to_string(), r);
            }
        }
    }
    let mut s = String::from("## Per-label running summary\n\n");
    if latest.is_empty() {
        s.push_str("(no `running_summary` rows in this trace)\n\n");
        return s;
    }
    s.push_str("| label | n | p50 µs | p95 µs | p99 µs | max µs | stalls | stalls100 |\n");
    s.push_str("|---|---:|---:|---:|---:|---:|---:|---:|\n");
    let mut sorted: Vec<(&String, &&TraceRow)> = latest.iter().collect();
    sorted.sort_by(|(a, _), (b, _)| a.cmp(b));
    for (lbl, row) in sorted {
        let n = field(&row.details, "n").unwrap_or("0");
        let p50 = field(&row.details, "p50_us").unwrap_or("0");
        let p95 = field(&row.details, "p95_us").unwrap_or("0");
        let p99 = field(&row.details, "p99_us").unwrap_or("0");
        let max = field(&row.details, "max_us").unwrap_or("0");
        let stalls = field(&row.details, "stalls").unwrap_or("0");
        let stalls100 = field(&row.details, "stalls100").unwrap_or("0");
        s.push_str(&format!(
            "| {lbl} | {n} | {p50} | {p95} | {p99} | {max} | {stalls} | {stalls100} |\n"
        ));
    }
    s.push('\n');
    s
}

fn stalls_section(rows: &[&TraceRow]) -> String {
    let stalls: Vec<&&TraceRow> = rows
        .iter()
        .filter(|r| r.kind == "stall" || r.kind == "stall100")
        .collect();
    let mut s = format!("## Stalls (n={})\n\n", stalls.len());
    if stalls.is_empty() {
        s.push_str("(no stall lines)\n\n");
        return s;
    }
    s.push_str("| t (ms) | kind | label | µs | details |\n");
    s.push_str("|---:|---|---|---:|---|\n");
    for row in stalls.iter().take(40) {
        let detail = truncate(&row.details, 80);
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            row.ms_since_start, row.kind, row.label, row.duration_us, detail
        ));
    }
    if stalls.len() > 40 {
        s.push_str(&format!(
            "_…and {} more stall rows omitted._\n",
            stalls.len() - 40
        ));
    }
    s.push('\n');
    s
}

fn top_slowest_section(rows: &[&TraceRow], k: usize) -> String {
    let mut by_dur: Vec<&&TraceRow> = rows
        .iter()
        .filter(|r| !matches!(r.kind.as_str(), "stall" | "stall100"))
        .filter(|r| r.duration_us > 0)
        .collect();
    by_dur.sort_by(|a, b| b.duration_us.cmp(&a.duration_us));
    let mut s = format!("## Top {k} slowest events\n\n");
    if by_dur.is_empty() {
        s.push_str("(no events with non-zero duration)\n\n");
        return s;
    }
    s.push_str("| t (ms) | kind | label | µs |\n");
    s.push_str("|---:|---|---|---:|\n");
    for row in by_dur.iter().take(k) {
        s.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            row.ms_since_start, row.kind, row.label, row.duration_us
        ));
    }
    s.push('\n');
    s
}

fn cold_paint_section(rows: &[&TraceRow]) -> String {
    let mut s = String::from("## Cold-paint signature\n\n");
    let paints: Vec<&&TraceRow> = rows
        .iter()
        .filter(|r| r.label.starts_with("frame_display:") || r.label == "row_count_walker")
        .collect();
    if paints.is_empty() {
        s.push_str("(no cold-paint labels found)\n\n");
        return s;
    }
    s.push_str("First 20 frame-display / walker entries (chronological):\n\n");
    s.push_str("| t (ms) | label | µs | details (truncated) |\n");
    s.push_str("|---:|---|---:|---|\n");
    for row in paints.iter().take(20) {
        let detail = truncate(&row.details, 60);
        s.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            row.ms_since_start, row.label, row.duration_us, detail
        ));
    }
    s.push('\n');

    // Slowest-lines roundup: pull every `row_count_slowest_lines` row,
    // tally by line index.
    let mut line_tally: BTreeMap<u32, u64> = BTreeMap::new();
    for row in rows.iter().filter(|r| r.label == "row_count_slowest_lines") {
        for token in row.details.split_whitespace() {
            if let Some(rest) = token.strip_prefix("line=") {
                if let Ok(idx) = rest.parse::<u32>() {
                    *line_tally.entry(idx).or_default() += 1;
                }
            }
        }
    }
    if !line_tally.is_empty() {
        s.push_str("Most-frequent slow source lines:\n\n");
        let mut sorted: Vec<(u32, u64)> = line_tally.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1));
        s.push_str("| line | appearances in slowest-N |\n");
        s.push_str("|---:|---:|\n");
        for (idx, count) in sorted.iter().take(10) {
            s.push_str(&format!("| {idx} | {count} |\n"));
        }
        s.push('\n');
    }
    s
}

fn edit_seq_section(rows: &[TraceRow], filters: &Filters) -> String {
    let mut s = String::from("## Edit-seq cross-thread timeline\n\n");
    let target_seq = filters.edit_seq;
    // Group rows by edit_seq.
    let mut by_seq: BTreeMap<u64, Vec<&TraceRow>> = BTreeMap::new();
    for r in rows {
        if let Some(seq) = field(&r.details, "edit_seq").and_then(|v| v.parse::<u64>().ok()) {
            if target_seq.is_none_or(|t| t == seq) {
                by_seq.entry(seq).or_default().push(r);
            }
        }
    }
    if by_seq.is_empty() {
        s.push_str("(no `edit_seq=` rows found)\n\n");
        return s;
    }
    if target_seq.is_none() {
        s.push_str(&format!(
            "{} edits with stamped seq numbers. Use `--edit-seq <N>` to inspect one.\n\n",
            by_seq.len()
        ));
        // Show summary: per-seq label set + spread.
        s.push_str("| edit_seq | rows | labels |\n");
        s.push_str("|---:|---:|---|\n");
        let mut sorted: Vec<(u64, Vec<&TraceRow>)> = by_seq.into_iter().collect();
        sorted.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
        for (seq, rs) in sorted.iter().take(20) {
            let mut labels: Vec<&str> = rs.iter().map(|r| r.label.as_str()).collect();
            labels.sort();
            labels.dedup();
            s.push_str(&format!(
                "| {seq} | {} | {} |\n",
                rs.len(),
                truncate(&labels.join(","), 80)
            ));
        }
        s.push('\n');
    } else if let (Some(rs), Some(seq)) = (by_seq.values().next(), target_seq) {
        s.push_str(&format!(
            "All rows tagged `edit_seq={seq}` (chronological):\n\n"
        ));
        s.push_str("| t (ms) | kind | label | µs | details |\n");
        s.push_str("|---:|---|---|---:|---|\n");
        for row in rs {
            let detail = truncate(&row.details, 80);
            s.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                row.ms_since_start, row.kind, row.label, row.duration_us, detail
            ));
        }
        s.push('\n');
    }
    s
}

fn worker_queue_section(rows: &[TraceRow]) -> String {
    let mut s = String::from("## Projection-worker queue depth\n\n");
    let depths: Vec<u64> = rows
        .iter()
        .filter(|r| r.label == "projection_worker_queue_depth")
        .filter_map(|r| field(&r.details, "depth").and_then(|v| v.parse::<u64>().ok()))
        .collect();
    if depths.is_empty() {
        s.push_str("(no queue-depth samples)\n\n");
        return s;
    }
    let max = depths.iter().max().copied().unwrap_or(0);
    let saturated = depths.iter().filter(|&&d| d > 0).count();
    s.push_str(&format!(
        "samples={} max_depth={} samples_with_pending={}\n\n",
        depths.len(),
        max,
        saturated
    ));
    s
}

fn process_state_section(rows: &[TraceRow]) -> String {
    let mut s = String::from("## Process state\n\n");
    let samples: Vec<&TraceRow> = rows.iter().filter(|r| r.label == "process_state").collect();
    if samples.is_empty() {
        s.push_str("(no `process_state` rows — long-running session timer didn't fire)\n\n");
        return s;
    }
    let extract = |key: &str| -> Vec<u64> {
        samples
            .iter()
            .filter_map(|r| field(&r.details, key).and_then(|v| v.parse::<u64>().ok()))
            .collect()
    };
    let ws = extract("ws_bytes");
    let peak = extract("peak_ws_bytes");
    let private_bytes = extract("private_bytes");
    let private_bytes_hwm = extract("private_bytes_hwm");
    let handles = extract("handles");
    s.push_str(&format!("Samples: {}\n\n", samples.len()));
    if !ws.is_empty() {
        s.push_str(&format!(
            "- Working set bytes: first={} last={} min={} max={}\n",
            ws.first().unwrap_or(&0),
            ws.last().unwrap_or(&0),
            ws.iter().min().unwrap_or(&0),
            ws.iter().max().unwrap_or(&0),
        ));
    }
    if !private_bytes.is_empty() {
        s.push_str(&format!(
            "- Private bytes: first={} last={} min={} max={}\n",
            private_bytes.first().unwrap_or(&0),
            private_bytes.last().unwrap_or(&0),
            private_bytes.iter().min().unwrap_or(&0),
            private_bytes.iter().max().unwrap_or(&0),
        ));
    }
    if let (Some(first), Some(last)) = (private_bytes_hwm.first(), private_bytes_hwm.last()) {
        s.push_str(&format!("- Private bytes HWM: first={first} last={last}\n"));
    }
    if let (Some(first), Some(last)) = (peak.first(), peak.last()) {
        s.push_str(&format!("- Peak working set: first={first} last={last}\n",));
    }
    if !handles.is_empty() {
        s.push_str(&format!(
            "- OS handles: first={} last={} max={}\n",
            handles.first().unwrap_or(&0),
            handles.last().unwrap_or(&0),
            handles.iter().max().unwrap_or(&0),
        ));
    }
    if let (Some(first), Some(last)) = (samples.first(), samples.last()) {
        let first_ac = field(&first.details, "ac").unwrap_or("?");
        let last_ac = field(&last.details, "ac").unwrap_or("?");
        let first_battery = field(&first.details, "battery_pct").unwrap_or("?");
        let last_battery = field(&last.details, "battery_pct").unwrap_or("?");
        let first_saver = field(&first.details, "saver").unwrap_or("?");
        let last_saver = field(&last.details, "saver").unwrap_or("?");
        s.push_str(&format!(
            "- Power: ac {first_ac}->{last_ac}, battery_pct {first_battery}->{last_battery}, saver {first_saver}->{last_saver}\n"
        ));
    }
    s.push('\n');
    s
}

pub(crate) fn truncate(s: &str, limit: usize) -> String {
    if s.len() <= limit {
        return s.to_string();
    }
    let mut t = String::with_capacity(limit + 1);
    for (i, ch) in s.chars().enumerate() {
        if i >= limit {
            t.push('…');
            break;
        }
        t.push(ch);
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canonical_row() {
        let row = parse_row("123\tevent\tedit_apply\t456\tedit_seq=7 kind=insert_text").unwrap();
        assert_eq!(row.ms_since_start, 123);
        assert_eq!(row.kind, "event");
        assert_eq!(row.label, "edit_apply");
        assert_eq!(row.duration_us, 456);
        assert_eq!(field(&row.details, "edit_seq"), Some("7"));
        assert_eq!(field(&row.details, "kind"), Some("insert_text"));
    }

    #[test]
    fn truncates_at_limit() {
        assert_eq!(truncate("hello", 10), "hello");
        let long = "abcdefghij".repeat(5);
        let t = truncate(&long, 12);
        assert!(t.ends_with('…'));
        assert!(t.chars().count() <= 13);
    }

    #[test]
    fn builds_minimal_report_without_panicking() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t.tsv");
        std::fs::write(
            &path,
            "0\tevent\ttrace_open\t0\tpath=test\n\
             10\tevent\tedit_apply\t250\tedit_seq=1 kind=insert\n\
             20\tstall\trow_count_walker\t30000\tlines=9000\n",
        )
        .unwrap();
        let filters = Filters::default();
        let report = build_report(&path, &filters).unwrap();
        assert!(report.contains("Metadata"));
        assert!(report.contains("Stalls"));
        assert!(report.contains("row_count_walker"));
    }
}
