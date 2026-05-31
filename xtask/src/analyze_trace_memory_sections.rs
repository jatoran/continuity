//! Memory-instrumentation sections for `cargo xtask analyze-trace`.
//!
//! Hosts the per-subsystem `event:memory_breakdown` table (including the
//! `### Graphics` and `### Accounting (private_bytes attribution)`
//! subsections), the `event:buffer_focus_change` rollups, and the
//! optional `event:decoration_cache_top` snapshot. Pulled out of
//! [`crate::analyze_trace_sections`] so both host files stay under the
//! 600-line conventions cap.

use crate::analyze_trace::{field, truncate, TraceRow};

/// Surface `event:buffer_focus_change` rows so the analyzer can answer
/// "which buffer switch did the user perform, and did the destination
/// buffer's decorations / tree cache survive?". Limited to the most
/// recent 20 entries to keep the report scan-friendly.
pub(crate) fn buffer_focus_events_section(rows: &[TraceRow]) -> String {
    let mut s = String::from("## Buffer focus events\n\n");
    let events: Vec<&TraceRow> = rows
        .iter()
        .filter(|r| r.label == "buffer_focus_change")
        .collect();
    if events.is_empty() {
        s.push_str("(no `event:buffer_focus_change` rows)\n\n");
        return s;
    }
    s.push_str(&format!("Total focus events: {}\n\n", events.len()));
    s.push_str("| t (ms) | from_buffer | to_buffer | from_pane | to_pane | dec_hit | tree_hit |\n");
    s.push_str("|---:|---|---|---|---|---|---|\n");
    let len = events.len();
    for row in events.iter().skip(len.saturating_sub(20)) {
        let from_buffer = field(&row.details, "from_buffer").unwrap_or("?");
        let to_buffer = field(&row.details, "to_buffer").unwrap_or("?");
        let from_pane = field(&row.details, "from_pane").unwrap_or("?");
        let to_pane = field(&row.details, "to_pane").unwrap_or("?");
        let dec_hit = field(&row.details, "decoration_cache_hit").unwrap_or("?");
        let tree_hit = field(&row.details, "tree_cache_hit").unwrap_or("?");
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            row.ms_since_start,
            truncate(from_buffer, 14),
            truncate(to_buffer, 14),
            from_pane,
            to_pane,
            dec_hit,
            tree_hit,
        ));
    }
    s.push('\n');
    s
}

/// Pull from `event:memory_breakdown` rows for per-subsystem
/// attribution alongside the `process_state` section.
///
/// Fields are grouped (trees, decorations, ropes, segment cache, layout
/// cache, undo, process state, graphics) so a reader scanning the table
/// can find the bucket they're investigating quickly. Each field is
/// silently skipped when older traces don't carry it. The closing
/// `### Accounting (private_bytes attribution)` subsection makes the
/// "private_bytes minus sum-of-known" gap explicit.
pub(crate) fn memory_breakdown_section(rows: &[TraceRow]) -> String {
    let mut s = String::from("## Memory breakdown (per-subsystem)\n\n");
    let samples: Vec<&TraceRow> = rows
        .iter()
        .filter(|r| r.label == "memory_breakdown")
        .collect();
    if samples.is_empty() {
        s.push_str(
            "(no `memory_breakdown` rows — run with `CONTINUITY_TRACE_SUMMARY_MS` \
             ≥ 250 ms enabled; flushes every 2 s by default)\n\n",
        );
        return s;
    }
    let extract = |key: &str| -> Vec<u64> {
        samples
            .iter()
            .filter_map(|r| field(&r.details, key).and_then(|v| v.parse::<u64>().ok()))
            .collect()
    };
    s.push_str(&format!("Samples: {}\n\n", samples.len()));
    s.push_str("### Rope generations (leak indicators)\n\n");
    s.push_str("| field | first | last | min | max |\n");
    s.push_str("|---|---:|---:|---:|---:|\n");
    write_field_rows(
        &mut s,
        &extract,
        &[
            "rope_generations_live",
            "rope_generations_live_hwm",
            "rope_snapshots_live",
            "rope_snapshots_live_hwm",
        ],
    );
    s.push_str("\n### Trees (per-worker BufferTreeCache aggregate)\n\n");
    s.push_str("| field | first | last | min | max |\n");
    s.push_str("|---|---:|---:|---:|---:|\n");
    write_field_rows(
        &mut s,
        &extract,
        &[
            // `tree_sitter_heap_bytes` is the exact tree-sitter C heap
            // (counting allocator); `tree_cache_bytes` is the older
            // `descendant_count * 64` lower-bound proxy, kept for contrast.
            "tree_sitter_heap_bytes",
            "tree_sitter_heap_bytes_hwm",
            "tree_cache_bytes",
            "tree_cache_bytes_hwm",
        ],
    );
    s.push_str("\n### Decorations\n\n");
    s.push_str("| field | first | last | min | max |\n");
    s.push_str("|---|---:|---:|---:|---:|\n");
    write_field_rows(
        &mut s,
        &extract,
        &[
            "decoration_cache_bytes",
            "decoration_cache_bytes_hwm",
            "decoration_cache_entries",
            "decoration_cache_entries_hwm",
            "decoration_cache_hits",
            "decoration_cache_misses",
            "decoration_cache_evictions",
        ],
    );
    s.push_str("\n### Ropes / buffers\n\n");
    s.push_str("| field | first | last | min | max |\n");
    s.push_str("|---|---:|---:|---:|---:|\n");
    write_field_rows(
        &mut s,
        &extract,
        &[
            "rope_bytes",
            "rope_bytes_hwm",
            "snapshot_history_bytes",
            "snapshot_history_bytes_hwm",
        ],
    );
    s.push_str("\n### Segment cache (walker)\n\n");
    s.push_str("| field | first | last | min | max |\n");
    s.push_str("|---|---:|---:|---:|---:|\n");
    write_field_rows(
        &mut s,
        &extract,
        &[
            "segment_cache_bytes",
            "segment_cache_bytes_hwm",
            "segment_cache_entries",
            "segment_cache_entries_hwm",
            "segment_cache_hits",
            "segment_cache_misses",
            "segment_cache_evictions",
            "run_cache_bytes",
            "run_cache_bytes_hwm",
            "wrap_cache_bytes",
            "wrap_cache_bytes_hwm",
        ],
    );
    s.push_str("\n### Layout cache (IDWriteTextLayout)\n\n");
    s.push_str("| field | first | last | min | max |\n");
    s.push_str("|---|---:|---:|---:|---:|\n");
    write_field_rows(
        &mut s,
        &extract,
        &[
            "layout_cache_entries",
            "layout_cache_entries_hwm",
            "layout_cache_capacity",
            "layout_cache_bytes",
            "layout_cache_bytes_hwm",
        ],
    );
    s.push_str("\n### Undo\n\n");
    s.push_str("| field | first | last | min | max |\n");
    s.push_str("|---|---:|---:|---:|---:|\n");
    write_field_rows(
        &mut s,
        &extract,
        &[
            "undo_tree_bytes",
            "undo_tree_bytes_hwm",
            "undo_tree_records",
            "undo_tree_records_hwm",
            "undo_tree_groups",
            "undo_tree_groups_hwm",
        ],
    );
    s.push_str("\n### Process state (persistence, projection, images)\n\n");
    s.push_str("| field | first | last | min | max |\n");
    s.push_str("|---|---:|---:|---:|---:|\n");
    write_field_rows(
        &mut s,
        &extract,
        &[
            "image_cache_bytes",
            "image_cache_bytes_hwm",
            "persist_unflushed_bytes",
            "persist_unflushed_bytes_hwm",
            "projection_queue_depth",
            "projection_queue_depth_hwm",
            "projection_queue_capacity",
        ],
    );
    s.push_str("\n### Graphics (DirectWrite / D3D / GPU)\n\n");
    s.push_str("| field | first | last | min | max |\n");
    s.push_str("|---|---:|---:|---:|---:|\n");
    write_field_rows(
        &mut s,
        &extract,
        &[
            "dwrite_owned_cache_bytes",
            "dwrite_owned_cache_bytes_hwm",
            "gpu_local_bytes",
            "gpu_local_bytes_hwm",
            "gpu_nonlocal_bytes",
            "gpu_nonlocal_bytes_hwm",
            "swapchain_bytes",
            "swapchain_bytes_hwm",
        ],
    );
    s.push_str(&memory_accounting_subsection(&extract, rows));
    s.push('\n');
    s
}

/// Bytes that count toward `private_bytes` (CPU-side commit charge). The
/// per-subsystem caches we attribute, all CPU-resident. The CPU-side
/// graphics figures (`dwrite_owned_cache_bytes`, the system-memory copy
/// of `swapchain_bytes`) are noted in the report text but not summed
/// here: `dwrite_owned_cache_bytes` is always 0 today and the swap-chain
/// system copy is implementation-defined. `gpu_local_bytes` /
/// `gpu_nonlocal_bytes` are GPU/adapter memory and are intentionally
/// excluded.
const ACCOUNTED_FIELDS: &[&str] = &[
    // Use the exact tree-sitter heap, not the `tree_cache_bytes` proxy
    // (lower-bound, ~1.5-3x undercount) — and never both, or the tree
    // would be double-counted. Traces predating the counting allocator
    // lack this field and undercount the tree in accounting; that is
    // acceptable for historical traces.
    "tree_sitter_heap_bytes",
    "decoration_cache_bytes",
    "rope_bytes",
    "segment_cache_bytes",
    "run_cache_bytes",
    "wrap_cache_bytes",
    "layout_cache_bytes",
    "undo_tree_bytes",
    "image_cache_bytes",
    "persist_unflushed_bytes",
];

/// `### Accounting (private_bytes attribution)` subsection.
///
/// Makes the "private_bytes minus sum-of-known" gap explicit. Computes
/// both the absolute attribution (last sample of each known field vs.
/// `private_bytes`) and the growth attribution (last − first), since the
/// cold-start baseline dominates absolute numbers while the per-buffer
/// story lives in the growth. Robust to an absent `private_bytes`
/// (older traces) — emits `n/a` for the affected rows.
fn memory_accounting_subsection<F>(extract: &F, rows: &[TraceRow]) -> String
where
    F: Fn(&str) -> Vec<u64>,
{
    let mut s = String::from("\n### Accounting (private_bytes attribution)\n\n");

    // Sum the LAST and FIRST sample of every attributed field.
    let mut sum_known_last: u64 = 0;
    let mut sum_known_first: u64 = 0;
    for field_name in ACCOUNTED_FIELDS {
        let series = extract(field_name);
        sum_known_last = sum_known_last.saturating_add(series.last().copied().unwrap_or(0));
        sum_known_first = sum_known_first.saturating_add(series.first().copied().unwrap_or(0));
    }

    // `private_bytes` comes from the `process_state` event, not
    // `memory_breakdown`. Absent in older traces → report n/a.
    let private_series: Vec<u64> = rows
        .iter()
        .filter(|r| r.label == "process_state")
        .filter_map(|r| field(&r.details, "private_bytes").and_then(|v| v.parse::<u64>().ok()))
        .collect();

    s.push_str("| metric | bytes |\n");
    s.push_str("|---|---:|\n");
    s.push_str(&format!("| sum_of_known_bytes | {sum_known_last} |\n"));
    match (private_series.first(), private_series.last()) {
        (Some(_), Some(private_last)) => {
            let residual = private_last.saturating_sub(sum_known_last);
            let accounted_pct = if *private_last > 0 {
                (sum_known_last as f64 / *private_last as f64) * 100.0
            } else {
                0.0
            };
            s.push_str(&format!("| private_bytes | {private_last} |\n"));
            s.push_str(&format!("| residual_bytes | {residual} |\n"));
            s.push_str(&format!("| accounted_pct | {accounted_pct:.1}% |\n"));
        }
        _ => {
            s.push_str("| private_bytes | n/a |\n");
            s.push_str("| residual_bytes | n/a |\n");
            s.push_str("| accounted_pct | n/a |\n");
        }
    }

    s.push_str("\nGrowth (last − first; the baseline dominates absolutes, the per-buffer story is in the growth):\n\n");
    s.push_str("| metric | bytes |\n");
    s.push_str("|---|---:|\n");
    let sum_known_growth = sum_known_last.saturating_sub(sum_known_first);
    s.push_str(&format!("| sum_of_known_growth | {sum_known_growth} |\n"));
    match (private_series.first(), private_series.last()) {
        (Some(private_first), Some(private_last)) => {
            let private_growth = private_last.saturating_sub(*private_first);
            let residual_growth = private_growth.saturating_sub(sum_known_growth);
            let accounted_growth_pct = if private_growth > 0 {
                (sum_known_growth as f64 / private_growth as f64) * 100.0
            } else {
                0.0
            };
            s.push_str(&format!("| private_bytes_growth | {private_growth} |\n"));
            s.push_str(&format!("| residual_growth | {residual_growth} |\n"));
            s.push_str(&format!(
                "| accounted_growth_pct | {accounted_growth_pct:.1}% |\n"
            ));
        }
        _ => {
            s.push_str("| private_bytes_growth | n/a |\n");
            s.push_str("| residual_growth | n/a |\n");
            s.push_str("| accounted_growth_pct | n/a |\n");
        }
    }

    // GPU memory is reported separately: it is mostly NOT part of
    // `private_bytes`, so it must not be folded into the residual.
    let gpu_local = extract("gpu_local_bytes");
    let gpu_nonlocal = extract("gpu_nonlocal_bytes");
    if !gpu_local.is_empty() || !gpu_nonlocal.is_empty() {
        let local_last = gpu_local.last().copied().unwrap_or(0);
        let nonlocal_last = gpu_nonlocal.last().copied().unwrap_or(0);
        s.push_str(&format!(
            "\nGPU memory (not in private_bytes): local={local_last} nonlocal={nonlocal_last}\n"
        ));
    }

    // Note which graphics figures count toward private_bytes.
    s.push_str(
        "\n_CPU-side (counts toward private_bytes): the attributed caches above, \
         plus dwrite_owned_cache_bytes and any system-memory copy of swapchain_bytes. \
         GPU-side (does NOT count toward private_bytes): gpu_local_bytes (VRAM), \
         gpu_nonlocal_bytes (shared), and the GPU-resident swap-chain back buffers._\n",
    );

    s
}

fn write_field_rows<F>(s: &mut String, extract: &F, fields: &[&str])
where
    F: Fn(&str) -> Vec<u64>,
{
    for field_name in fields {
        let series = extract(field_name);
        if series.is_empty() {
            continue;
        }
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            field_name,
            series.first().unwrap_or(&0),
            series.last().unwrap_or(&0),
            series.iter().min().unwrap_or(&0),
            series.iter().max().unwrap_or(&0),
        ));
    }
}

/// Surface the most recent `event:decoration_cache_top` sample so the
/// analyzer can spot a single oversized buffer holding the
/// `decoration_cache_bytes` curve up. Section is omitted entirely when
/// the trace contains no `decoration_cache_top` rows (which is the
/// default — those rows only fire under
/// `CONTINUITY_TRACE_DECORATION_TOP=1`).
pub(crate) fn decoration_cache_top_section(rows: &[TraceRow]) -> String {
    let samples: Vec<&TraceRow> = rows
        .iter()
        .filter(|r| r.label == "decoration_cache_top")
        .collect();
    if samples.is_empty() {
        return String::new();
    }
    let mut s = String::from("## Decoration cache top entries\n\n");
    let latest = samples.last().expect("non-empty checked above");
    s.push_str(&format!(
        "Latest sample at t={} ms (of {} samples).\n\n",
        latest.ms_since_start,
        samples.len()
    ));
    s.push_str("| slot | buffer_id | bytes |\n");
    s.push_str("|---|---|---:|\n");
    for token in latest.details.split_whitespace() {
        if let Some((key, value)) = token.split_once('=') {
            if let Some(rest) = key.strip_prefix('e') {
                if rest.chars().all(|c| c.is_ascii_digit()) {
                    if let Some((bid, bytes)) = value.split_once(':') {
                        s.push_str(&format!("| {key} | {bid} | {bytes} |\n"));
                    }
                }
            }
        }
    }
    s.push('\n');
    s
}
