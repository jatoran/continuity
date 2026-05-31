//! Newer report sections for `cargo xtask analyze-trace`.
//!
//! Pulled out of [`crate::analyze_trace`] so the host file stays under
//! the 600-line convention cap. These sections all read the same
//! `TraceRow` slice the main module owns; they're free functions that
//! return `String` for the host to concatenate into the final report.

use std::collections::BTreeMap;

use crate::analyze_trace::{field, truncate, Filters, TraceRow};

const PAINT_CONTEXT_ROW_WINDOW: usize = 80;

fn nearest_detail(
    rows: &[TraceRow],
    index: usize,
    label: &str,
    max_rows: usize,
    max_detail_len: usize,
) -> Option<String> {
    rows.iter()
        .enumerate()
        .filter(|(candidate_index, row)| {
            row.label == label && candidate_index.abs_diff(index) <= max_rows
        })
        .min_by_key(|(candidate_index, _)| candidate_index.abs_diff(index))
        .map(|(_, row)| truncate(&row.details, max_detail_len))
}

/// For each `WM_PAINT` or `renderer.draw_buffer` over the stall
/// threshold, find the nearest `paint:render_stats` row in the same
/// paint cycle and surface its details alongside the slow paint.
/// The renderer emits the stats before the later paint timing marks,
/// so the search intentionally checks both sides. Walker stalls get
/// the same treatment paired with `row_count_walker_stats`.
pub(crate) fn worst_paints_section(rows: &[TraceRow]) -> String {
    const STALL_THRESHOLD_US: u64 = 16_000;
    let mut s = String::from("## Worst paints (with adjacent render_stats)\n\n");
    let slow: Vec<(usize, &TraceRow)> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            (r.kind == "wndproc" || r.kind == "paint")
                && (r.label == "WM_PAINT" || r.label == "renderer.draw_buffer")
                && r.duration_us >= STALL_THRESHOLD_US
        })
        .collect();
    if slow.is_empty() {
        s.push_str("(no paint stalls)\n\n");
        return s;
    }
    s.push_str("| t (ms) | label | µs | reason | render_stats | draw_stages |\n");
    s.push_str("|---:|---|---:|---|---|---|\n");
    for (idx, paint_row) in slow.iter().take(20) {
        let stats_detail = nearest_detail(
            rows,
            *idx,
            "paint:render_stats",
            PAINT_CONTEXT_ROW_WINDOW,
            200,
        )
        .unwrap_or_else(|| "<no render_stats nearby>".to_string());
        let stage_detail = nearest_detail(
            rows,
            *idx,
            "renderer_draw_stages",
            PAINT_CONTEXT_ROW_WINDOW,
            160,
        )
        .unwrap_or_else(|| "<no draw_stages nearby>".to_string());
        let reason = field(&paint_row.details, "reason").unwrap_or("?");
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            paint_row.ms_since_start,
            paint_row.label,
            paint_row.duration_us,
            reason,
            stats_detail,
            stage_detail,
        ));
    }
    let walker_slow: Vec<(usize, &TraceRow)> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.label == "row_count_walker" && r.duration_us >= STALL_THRESHOLD_US)
        .collect();
    if !walker_slow.is_empty() {
        s.push_str("\n### Walker stalls (with reason + stage breakdown)\n\n");
        s.push_str("| t (ms) | µs | reason | walker_stats |\n");
        s.push_str("|---:|---:|---|---|\n");
        for (idx, walker_row) in walker_slow.iter().take(10) {
            let walker_stats = rows
                .iter()
                .skip(*idx)
                .take(20)
                .find(|r| r.label == "row_count_walker_stats")
                .map(|r| truncate(&r.details, 200))
                .unwrap_or_else(|| "<no walker_stats nearby>".to_string());
            let reason = field(&walker_row.details, "reason").unwrap_or("?");
            s.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                walker_row.ms_since_start, walker_row.duration_us, reason, walker_stats,
            ));
        }
    }
    s.push('\n');
    s
}

/// Surface stack rows captured next to `stall100` events.
pub(crate) fn stall_stacks_section(rows: &[&TraceRow]) -> String {
    let stacks: Vec<&&TraceRow> = rows
        .iter()
        .filter(|r| r.label == "stall100_stack")
        .collect();
    let mut s = format!("## Stall stacks (n={})\n\n", stacks.len());
    if stacks.is_empty() {
        s.push_str("(no `stall100_stack` rows)\n\n");
        return s;
    }
    s.push_str("| t (ms) | stalled label | frames |\n");
    s.push_str("|---:|---|---|\n");
    for row in stacks.iter().take(20) {
        let label = field(&row.details, "label").unwrap_or("?");
        let frames = row
            .details
            .split_whitespace()
            .filter(|token| token.starts_with("frame_"))
            .take(8)
            .collect::<Vec<_>>()
            .join(" ");
        s.push_str(&format!(
            "| {} | {} | {} |\n",
            row.ms_since_start,
            label,
            truncate(&frames, 220),
        ));
    }
    s.push('\n');
    s
}

/// When no `--edit-seq` filter is set and the trace has edits,
/// auto-print the slowest edit's full stitched timeline as a sample.
pub(crate) fn edit_detail_auto_section(rows: &[TraceRow], filters: &Filters) -> String {
    let mut s = String::from("## Slowest edit (auto sample)\n\n");
    if filters.edit_seq.is_some() {
        s.push_str("(skipped — `--edit-seq` filter is active; see Edit-seq section above)\n\n");
        return s;
    }
    let slowest_edit = rows
        .iter()
        .filter(|r| r.label == "edit_apply" && r.duration_us > 0)
        .max_by_key(|r| r.duration_us);
    let Some(edit) = slowest_edit else {
        s.push_str("(no `edit_apply` events found)\n\n");
        return s;
    };
    let Some(seq) = field(&edit.details, "edit_seq").and_then(|v| v.parse::<u64>().ok()) else {
        s.push_str(&format!(
            "Slowest `edit_apply` at t={} ms ({} µs) has no edit_seq stamp.\n\n",
            edit.ms_since_start, edit.duration_us,
        ));
        return s;
    };
    s.push_str(&format!(
        "Slowest edit: `edit_seq={seq}` at t={} ms, total `edit_apply` = {} µs.\n\n",
        edit.ms_since_start, edit.duration_us,
    ));
    let stitched: Vec<&TraceRow> = rows
        .iter()
        .filter(|r| field(&r.details, "edit_seq").and_then(|v| v.parse::<u64>().ok()) == Some(seq))
        .collect();
    s.push_str("| t (ms) | kind | label | µs | details |\n");
    s.push_str("|---:|---|---|---:|---|\n");
    for row in &stitched {
        let detail = truncate(&row.details, 80);
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            row.ms_since_start, row.kind, row.label, row.duration_us, detail,
        ));
    }
    s.push('\n');
    s
}

// `memory_breakdown_section` (per-subsystem attribution + the
// `### Graphics` and `### Accounting (private_bytes attribution)`
// subsections), plus `buffer_focus_events_section` and
// `decoration_cache_top_section`, live in
// `analyze_trace_memory_sections.rs` so this file stays under the
// 600-line conventions cap.

/// User-action timeline. Reads `event:command_dispatch` rows produced
/// by `Registry::dispatch`. One bucket per `id=…` token so the
/// analyzer surfaces what the user actually triggered during the run.
pub(crate) fn command_timeline_section(rows: &[TraceRow]) -> String {
    let dispatches: Vec<&TraceRow> = rows
        .iter()
        .filter(|r| r.label == "command_dispatch")
        .collect();
    let mut s = String::from("## Command dispatch timeline\n\n");
    if dispatches.is_empty() {
        s.push_str("(no `event:command_dispatch` rows)\n\n");
        return s;
    }
    s.push_str(&format!("Total dispatches: {}\n\n", dispatches.len()));
    let mut by_id: BTreeMap<String, (u64, u64, u64, u64)> = BTreeMap::new();
    for row in &dispatches {
        let id = field(&row.details, "id").unwrap_or("?").to_string();
        let outcome_token = field(&row.details, "outcome").unwrap_or("?");
        let entry = by_id.entry(id).or_insert((0, 0, 0, 0));
        entry.0 += 1;
        entry.1 = entry.1.max(row.duration_us);
        if outcome_token == "ok" {
            entry.2 += 1;
        } else {
            entry.3 += 1;
        }
    }
    s.push_str("### Per-command totals\n\n");
    s.push_str("| command id | n | ok | err | max µs |\n");
    s.push_str("|---|---:|---:|---:|---:|\n");
    let mut sorted: Vec<(String, (u64, u64, u64, u64))> = by_id.into_iter().collect();
    sorted.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    for (id, (n, max_us, ok, err)) in sorted.iter().take(40) {
        s.push_str(&format!("| {id} | {n} | {ok} | {err} | {max_us} |\n"));
    }
    s.push_str("\n### Recent dispatches (last 30)\n\n");
    s.push_str("| t (ms) | id | outcome | µs |\n");
    s.push_str("|---:|---|---|---:|\n");
    let len = dispatches.len();
    for row in dispatches.iter().skip(len.saturating_sub(30)) {
        let id = field(&row.details, "id").unwrap_or("?");
        let outcome_token = field(&row.details, "outcome").unwrap_or("?");
        s.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            row.ms_since_start, id, outcome_token, row.duration_us,
        ));
    }
    s.push('\n');
    s
}

/// Tab / pane / window / buffer lifecycle. Reads
/// `event:tab_close`, `event:tab_reopen`, `event:smart_reopen`. Each
/// row is rendered with its key fields so the analyzer can answer
/// "which tab closed, where did it go on reopen, did the reopen
/// succeed?".
pub(crate) fn lifecycle_section(rows: &[TraceRow]) -> String {
    let mut s = String::from("## Tab / pane / window lifecycle\n\n");
    let lifecycle: Vec<&TraceRow> = rows
        .iter()
        .filter(|r| {
            matches!(
                r.label.as_str(),
                "tab_close"
                    | "tab_reopen"
                    | "pane_close"
                    | "smart_reopen"
                    | "buffer_adopt"
                    | "buffer_drop"
            )
        })
        .collect();
    if lifecycle.is_empty() {
        s.push_str("(no lifecycle events)\n\n");
        return s;
    }
    s.push_str(&format!("Total lifecycle events: {}\n\n", lifecycle.len()));
    s.push_str("| t (ms) | label | outcome / after | details |\n");
    s.push_str("|---:|---|---|---|\n");
    for row in lifecycle.iter().take(80) {
        let outcome = field(&row.details, "outcome")
            .or_else(|| field(&row.details, "after"))
            .unwrap_or("?");
        let detail = truncate(&row.details, 160);
        s.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            row.ms_since_start, row.label, outcome, detail,
        ));
    }
    s.push('\n');
    let mut tab_close = 0u64;
    let mut tab_close_cascade = 0u64;
    let mut pane_close = 0u64;
    let mut tab_reopen_origin = 0u64;
    let mut tab_reopen_resplit = 0u64;
    let mut tab_reopen_fallback = 0u64;
    let mut tab_reopen_phantom = 0u64;
    let mut tab_reopen_empty = 0u64;
    let mut smart_reopen_spawn = 0u64;
    let mut smart_reopen_delegate = 0u64;
    for row in &lifecycle {
        match row.label.as_str() {
            "tab_close" => {
                tab_close += 1;
                if field(&row.details, "after") == Some("pane_collapse_cascade") {
                    tab_close_cascade += 1;
                }
            }
            "pane_close" => pane_close += 1,
            "tab_reopen" => match field(&row.details, "outcome").unwrap_or("") {
                "ok" => match field(&row.details, "dest").unwrap_or("") {
                    "origin_pane" => tab_reopen_origin += 1,
                    "restored_via_resplit" => tab_reopen_resplit += 1,
                    _ => tab_reopen_fallback += 1,
                },
                "phantom_buffer_skip" => tab_reopen_phantom += 1,
                "empty_stack" | "exhausted_after_skips" => tab_reopen_empty += 1,
                _ => {}
            },
            "smart_reopen" => match field(&row.details, "outcome").unwrap_or("") {
                "spawn_ok" => smart_reopen_spawn += 1,
                "delegate_local" => smart_reopen_delegate += 1,
                _ => {}
            },
            _ => {}
        }
    }
    s.push_str("### Lifecycle counters\n\n");
    s.push_str(&format!(
        "- tab_close: {tab_close} (of which after=pane_collapse_cascade: {tab_close_cascade})\n"
    ));
    s.push_str(&format!("- pane_close: {pane_close}\n"));
    s.push_str(&format!(
        "- tab_reopen dest=origin_pane: {tab_reopen_origin}\n"
    ));
    s.push_str(&format!(
        "- tab_reopen dest=restored_via_resplit: {tab_reopen_resplit}\n"
    ));
    s.push_str(&format!(
        "- tab_reopen dest=fallback_* (origin collapsed and no resplit anchor): {tab_reopen_fallback}\n"
    ));
    s.push_str(&format!(
        "- tab_reopen outcome=phantom_buffer_skip: {tab_reopen_phantom}\n"
    ));
    s.push_str(&format!(
        "- tab_reopen outcome=empty_stack|exhausted_after_skips: {tab_reopen_empty}\n"
    ));
    s.push_str(&format!(
        "- smart_reopen outcome=spawn_ok: {smart_reopen_spawn}\n"
    ));
    s.push_str(&format!(
        "- smart_reopen outcome=delegate_local: {smart_reopen_delegate}\n\n"
    ));
    s
}

/// Closed-history stack push/pop log. Reads
/// `event:closed_history_push` / `event:closed_history_pop` rows so
/// the analyzer can correlate window-level closes with
/// `smart_reopen outcome=spawn_ok`.
pub(crate) fn closed_history_section(rows: &[TraceRow]) -> String {
    let mut s = String::from("## Closed-history stack\n\n");
    let events: Vec<&TraceRow> = rows
        .iter()
        .filter(|r| r.label == "closed_history_push" || r.label == "closed_history_pop")
        .collect();
    if events.is_empty() {
        s.push_str("(no closed-history events)\n\n");
        return s;
    }
    let pushes = events
        .iter()
        .filter(|r| r.label == "closed_history_push")
        .count();
    let pops = events
        .iter()
        .filter(|r| r.label == "closed_history_pop")
        .count();
    s.push_str(&format!("Pushes: {pushes}, Pops: {pops}\n\n"));
    s.push_str("| t (ms) | label | outcome | kind | payload_bytes |\n");
    s.push_str("|---:|---|---|---|---:|\n");
    for row in events.iter().take(60) {
        let outcome = field(&row.details, "outcome").unwrap_or("?");
        let kind = field(&row.details, "kind").unwrap_or("-");
        let payload = field(&row.details, "payload_bytes").unwrap_or("-");
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            row.ms_since_start, row.label, outcome, kind, payload,
        ));
    }
    s.push('\n');
    s
}

/// Summarize row-index cache miss reasons.
pub(crate) fn row_index_cache_section(rows: &[TraceRow]) -> String {
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for row in rows.iter().filter(|r| r.label == "row_index_cache") {
        if field(&row.details, "action") != Some("miss") {
            continue;
        }
        let reason = field(&row.details, "reason").unwrap_or("?").to_string();
        *counts.entry(reason).or_default() += 1;
    }
    let mut s = String::from("## Row-index cache misses\n\n");
    if counts.is_empty() {
        s.push_str("(no `event:row_index_cache action=miss` rows)\n\n");
        return s;
    }
    s.push_str("| reason | misses |\n");
    s.push_str("|---|---:|\n");
    for (reason, count) in counts {
        s.push_str(&format!("| {reason} | {count} |\n"));
    }
    s.push('\n');
    s
}
