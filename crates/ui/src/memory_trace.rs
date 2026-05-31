//! Per-subsystem memory snapshot for the trace stream.
//!
//! Companion to [`crate::process_trace`]. Where `process_trace` reports
//! Win32 process-level counters (working set, handles, GDI/USER),
//! this module reports continuity-internal subsystem sizes so a
//! trace consumer can attribute the process-level RSS delta to a
//! specific owner.
//!
//! Emitted as `event:memory_breakdown` at the same cadence as
//! `event:process_state` (driven by [`crate::paint_trace_summary::tick`]).
//!
//! Coverage includes UI caches (`LayoutCache`, row-count walker caches,
//! `DecorationCache`, `ImageCache`), the per-worker tree-sitter
//! [`continuity_decorate::BufferTreeCache`] aggregated across the
//! decoration pool, core-owned buffer memory (`rope_bytes`,
//! `snapshot_history_bytes` excluding the live rope, `undo_tree_bytes`),
//! the `RopeSnapshotRegistry` leak indicators, persistence backlog, and
//! projection-worker queue state.
//!
//! ## Where the walker `segment_cache_bytes` lives
//!
//! `walker_segment_cache` is an `Arc<continuity_display_map::SegmentCache>`
//! owned by `Window` (`crates/ui/src/window.rs::Window::walker_segment_cache`)
//! and shared with the projection worker. Its definition lives in
//! `crates/display_map/src/segment_cache.rs`.
//!
//! ## High-water-mark tracking
//!
//! Each numeric field that represents "size" (bytes, entries, counts)
//! is mirrored as `<name>_hwm`, where the HWM is a process-wide
//! `AtomicUsize` updated via `fetch_max`. Capacity / queue-capacity
//! fields are omitted from HWM tracking because they are configured
//! constants whose "max so far" carries no information.
//!
//! ## Decoration-cache top entries
//!
//! When `CONTINUITY_TRACE_DECORATION_TOP=1`, every flush additionally
//! emits an `event:decoration_cache_top` line listing the top 3 entries
//! by estimated byte size, formatted as
//! `n=3 e0=<bid>:<bytes> e1=<bid>:<bytes> e2=<bid>:<bytes>`.
//!
//! ## Graphics (DirectWrite / D3D / GPU) coverage
//!
//! - `dwrite_owned_cache_bytes` — DirectWrite-derived caches WE own and
//!   retain that are *not* already counted elsewhere. Today this is
//!   always `0`: every retained DWrite-derived cache (`LayoutCache`,
//!   `RunCache`, `WrapCache`, `SegmentCache`) is already counted under
//!   its own field, and the only other DWrite consumer
//!   (`render::DirectWriteWidthMeasure`) is a per-frame transient, not a
//!   retained cache. DirectWrite's *internal* font-table and glyph-run
//!   caches are not queryable through any public API; they live in
//!   DWrite's opaque heap and therefore land in the `private_bytes`
//!   residual rather than any attributed bucket.
//! - `gpu_local_bytes` / `gpu_nonlocal_bytes` — DXGI
//!   `IDXGIAdapter3::QueryVideoMemoryInfo` current usage for the local
//!   (VRAM) and non-local (shared) segment groups. These are *mostly
//!   not* part of `private_bytes` (they are GPU/adapter memory), so the
//!   report treats them as a separate informational line and does NOT
//!   add them to the residual's sum-of-known.
//! - `swapchain_bytes` — deterministic estimate of the swap-chain back
//!   buffers (`width × height × 4 × buffer_count`). GPU-resident; the
//!   system-memory copy (if any) is the only part that could touch
//!   `private_bytes`.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::Window;

/// One process-wide high-water-mark slot.
struct Hwm {
    name: &'static str,
    value: AtomicUsize,
}

impl Hwm {
    const fn new(name: &'static str) -> Self {
        Self {
            name,
            value: AtomicUsize::new(0),
        }
    }

    fn observe(&self, current: usize) -> usize {
        let mut prev = self.value.load(Ordering::Relaxed);
        while current > prev {
            match self.value.compare_exchange_weak(
                prev,
                current,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return current,
                Err(next) => prev = next,
            }
        }
        prev
    }
}

// Every "size-shaped" field gets a private HWM slot. Capacity fields
// (`layout_cache_capacity`, `projection_queue_capacity`) are intentionally
// omitted — they are configured constants and reporting "max seen"
// would only paper over a real signal.
static HWM_LAYOUT_CACHE_ENTRIES: Hwm = Hwm::new("layout_cache_entries");
static HWM_LAYOUT_CACHE_BYTES: Hwm = Hwm::new("layout_cache_bytes");
static HWM_RUN_CACHE_BYTES: Hwm = Hwm::new("run_cache_bytes");
static HWM_WRAP_CACHE_BYTES: Hwm = Hwm::new("wrap_cache_bytes");
static HWM_SEGMENT_CACHE_BYTES: Hwm = Hwm::new("segment_cache_bytes");
static HWM_SEGMENT_CACHE_ENTRIES: Hwm = Hwm::new("segment_cache_entries");
static HWM_ROPE_BYTES: Hwm = Hwm::new("rope_bytes");
static HWM_SNAPSHOT_HISTORY_BYTES: Hwm = Hwm::new("snapshot_history_bytes");
static HWM_DECORATION_CACHE_BYTES: Hwm = Hwm::new("decoration_cache_bytes");
static HWM_DECORATION_CACHE_ENTRIES: Hwm = Hwm::new("decoration_cache_entries");
static HWM_IMAGE_CACHE_BYTES: Hwm = Hwm::new("image_cache_bytes");
static HWM_UNDO_TREE_BYTES: Hwm = Hwm::new("undo_tree_bytes");
static HWM_UNDO_TREE_RECORDS: Hwm = Hwm::new("undo_tree_records");
static HWM_UNDO_TREE_GROUPS: Hwm = Hwm::new("undo_tree_groups");
static HWM_PERSIST_UNFLUSHED_BYTES: Hwm = Hwm::new("persist_unflushed_bytes");
static HWM_PROJECTION_QUEUE_DEPTH: Hwm = Hwm::new("projection_queue_depth");
static HWM_TREE_CACHE_BYTES: Hwm = Hwm::new("tree_cache_bytes");
static HWM_TREE_SITTER_HEAP_BYTES: Hwm = Hwm::new("tree_sitter_heap_bytes");
static HWM_ROPE_GENERATIONS_LIVE: Hwm = Hwm::new("rope_generations_live");
static HWM_ROPE_SNAPSHOTS_LIVE: Hwm = Hwm::new("rope_snapshots_live");
static HWM_DWRITE_OWNED_CACHE_BYTES: Hwm = Hwm::new("dwrite_owned_cache_bytes");
static HWM_GPU_LOCAL_BYTES: Hwm = Hwm::new("gpu_local_bytes");
static HWM_GPU_NONLOCAL_BYTES: Hwm = Hwm::new("gpu_nonlocal_bytes");
static HWM_SWAPCHAIN_BYTES: Hwm = Hwm::new("swapchain_bytes");

/// Emit one `event:memory_breakdown` line. No-op when tracing is
/// disabled. Driven by the running-summary flush timer.
pub(crate) fn emit_snapshot(window: &Window) {
    if !crate::paint_trace::is_trace_enabled() {
        return;
    }
    let snapshot = collect_snapshot(window);
    let detail = format_detail(&snapshot);
    crate::paint_trace::log_event("memory_breakdown", &detail);
    if decoration_top_enabled() {
        emit_decoration_cache_top(window);
    }
}

fn decoration_top_enabled() -> bool {
    std::env::var_os("CONTINUITY_TRACE_DECORATION_TOP")
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false)
}

fn emit_decoration_cache_top(window: &Window) {
    let entries = window.decoration_cache.largest_entries(3);
    if entries.is_empty() {
        return;
    }
    let mut detail = format!("n={}", entries.len());
    for (i, (buffer_id, bytes)) in entries.iter().enumerate() {
        use std::fmt::Write;
        let _ = write!(&mut detail, " e{i}={buffer_id:#x}:{bytes}");
    }
    crate::paint_trace::log_event("decoration_cache_top", &detail);
}

/// Numeric snapshot of every field surfaced on `event:memory_breakdown`.
/// Pulled out of [`emit_snapshot`] so the same numbers can drive both
/// the TSV emit and (eventually) downstream consumers without
/// re-querying `Window`.
struct MemorySnapshot {
    layout_cache_entries: usize,
    layout_cache_capacity: usize,
    layout_cache_bytes: usize,
    run_cache_bytes: usize,
    wrap_cache_bytes: usize,
    segment_cache_bytes: usize,
    segment_cache_entries: usize,
    segment_cache_hits: u64,
    segment_cache_misses: u64,
    segment_cache_evictions: u64,
    rope_bytes: usize,
    snapshot_history_bytes: usize,
    decoration_cache_bytes: usize,
    decoration_cache_entries: usize,
    decoration_cache_hits: u64,
    decoration_cache_misses: u64,
    decoration_cache_evictions: u64,
    tree_cache_bytes: usize,
    /// Exact live tree-sitter C heap, process-wide (counting allocator).
    /// Unlike `tree_cache_bytes` (a `descendant_count * 64` lower-bound
    /// proxy), this is the real number — the honest tree attribution.
    tree_sitter_heap_bytes: usize,
    undo_tree_bytes: usize,
    undo_tree_records: usize,
    undo_tree_groups: usize,
    image_cache_bytes: usize,
    persist_unflushed_bytes: usize,
    projection_queue_depth: usize,
    projection_queue_capacity: usize,
    rope_generations_live: usize,
    rope_snapshots_live: usize,
    /// DirectWrite-derived caches we own and retain that are not already
    /// counted under another field. Always `0` today — see the module
    /// docs "Graphics" section; DWrite's internal caches are not
    /// queryable and live in the residual.
    dwrite_owned_cache_bytes: usize,
    /// DXGI local (VRAM) segment-group current usage, bytes. `0` when
    /// the renderer is absent or the adapter predates `IDXGIAdapter3`.
    gpu_local_bytes: u64,
    /// DXGI non-local (shared system) segment-group current usage,
    /// bytes. `0` when unavailable.
    gpu_nonlocal_bytes: u64,
    /// Deterministic swap-chain back-buffer byte estimate. `0` when the
    /// renderer is absent.
    swapchain_bytes: u64,
}

fn collect_snapshot(window: &Window) -> MemorySnapshot {
    let layout_cache_entries = window.cache.len();
    let layout_cache_capacity = window.cache.capacity();
    let layout_cache_bytes = window.cache.byte_size_estimate();
    let run_cache_bytes = window.walker_run_cache.byte_size_estimate();
    let wrap_cache_bytes = window.walker_wrap_cache.byte_size_estimate();
    let segment_cache_bytes = window.walker_segment_cache.byte_size_estimate();
    let segment_cache_entries = window.walker_segment_cache.len();
    let segment_counters = window.walker_segment_cache.counters();
    let core_memory = window.editor.memory_stats();
    let decoration_cache_bytes = window.decoration_cache.byte_size_estimate();
    let decoration_cache_entries = window.decoration_cache.len();
    let decoration_counters = window.decoration_cache.counters();
    let tree_cache_bytes = window
        .decorate_pool
        .as_ref()
        .map(|p| p.tree_cache_bytes_estimate())
        .unwrap_or(0);
    let tree_sitter_heap_bytes = continuity_decorate::tree_sitter_heap_bytes();
    let image_cache_bytes = window
        .renderer
        .as_ref()
        .map(|r| r.image_cache_current_bytes())
        .unwrap_or(0);
    // Graphics accounting. `dwrite_owned_cache_bytes` is constant 0:
    // every retained DWrite-derived cache is already counted under its
    // own field and DWrite's internal caches are not queryable (they
    // live in the residual — see module docs). GPU figures come from
    // DXGI `QueryVideoMemoryInfo`; the swap-chain estimate is
    // deterministic from the back-buffer dimensions.
    let dwrite_owned_cache_bytes = 0usize;
    let (gpu_local_bytes, gpu_nonlocal_bytes, swapchain_bytes) = window
        .renderer
        .as_ref()
        .map(|r| {
            let gpu = r.gpu_memory_info();
            (
                gpu.local_bytes,
                gpu.nonlocal_bytes,
                r.swapchain_bytes_estimate(),
            )
        })
        .unwrap_or((0, 0, 0));
    let persist_unflushed_bytes = window
        .persist_client
        .as_ref()
        .map(|c| c.unflushed_bytes())
        .unwrap_or(0);
    let (projection_queue_depth, projection_queue_capacity) = window
        .projection_worker
        .as_ref()
        .map(|w| (w.queue_depth(), w.queue_capacity()))
        .unwrap_or((0, 0));
    let snapshot_registry = continuity_buffer::RopeSnapshotRegistry::instance();
    MemorySnapshot {
        layout_cache_entries,
        layout_cache_capacity,
        layout_cache_bytes,
        run_cache_bytes,
        wrap_cache_bytes,
        segment_cache_bytes,
        segment_cache_entries,
        segment_cache_hits: segment_counters.hits,
        segment_cache_misses: segment_counters.misses,
        segment_cache_evictions: segment_counters.evictions,
        rope_bytes: core_memory.rope_bytes,
        snapshot_history_bytes: core_memory.snapshot_history_bytes,
        decoration_cache_bytes,
        decoration_cache_entries,
        decoration_cache_hits: decoration_counters.hits,
        decoration_cache_misses: decoration_counters.misses,
        decoration_cache_evictions: decoration_counters.evictions,
        tree_cache_bytes,
        tree_sitter_heap_bytes,
        undo_tree_bytes: core_memory.undo_tree_bytes,
        undo_tree_records: core_memory.undo_tree_records,
        undo_tree_groups: core_memory.undo_tree_groups,
        image_cache_bytes,
        persist_unflushed_bytes,
        projection_queue_depth,
        projection_queue_capacity,
        rope_generations_live: snapshot_registry.distinct_arc_heads(),
        rope_snapshots_live: snapshot_registry.live_snapshot_count(),
        dwrite_owned_cache_bytes,
        gpu_local_bytes,
        gpu_nonlocal_bytes,
        swapchain_bytes,
    }
}

fn format_detail(s: &MemorySnapshot) -> String {
    use std::fmt::Write;
    let mut buf = String::with_capacity(1024);

    // Helper: emit `name=value name_hwm=hwm`, updating the matching slot.
    macro_rules! field_hwm {
        ($buf:expr, $hwm:expr, $value:expr) => {{
            let v = $value as usize;
            let hwm = $hwm.observe(v);
            let _ = write!($buf, " {}={} {}_hwm={}", $hwm.name, v, $hwm.name, hwm);
        }};
    }
    macro_rules! field_plain {
        ($buf:expr, $name:expr, $value:expr) => {{
            let _ = write!($buf, " {}={}", $name, $value);
        }};
    }

    // Strip the leading space on the first write by writing the first
    // entry without one, then macro-emitting the rest.
    let _ = write!(
        &mut buf,
        "layout_cache_entries={} layout_cache_entries_hwm={}",
        s.layout_cache_entries,
        HWM_LAYOUT_CACHE_ENTRIES.observe(s.layout_cache_entries),
    );
    // Capacity fields: no HWM (configured constants).
    field_plain!(&mut buf, "layout_cache_capacity", s.layout_cache_capacity);
    field_hwm!(&mut buf, HWM_LAYOUT_CACHE_BYTES, s.layout_cache_bytes);
    field_hwm!(&mut buf, HWM_RUN_CACHE_BYTES, s.run_cache_bytes);
    field_hwm!(&mut buf, HWM_WRAP_CACHE_BYTES, s.wrap_cache_bytes);
    field_hwm!(&mut buf, HWM_SEGMENT_CACHE_BYTES, s.segment_cache_bytes);
    field_hwm!(&mut buf, HWM_SEGMENT_CACHE_ENTRIES, s.segment_cache_entries);
    field_plain!(&mut buf, "segment_cache_hits", s.segment_cache_hits);
    field_plain!(&mut buf, "segment_cache_misses", s.segment_cache_misses);
    field_plain!(
        &mut buf,
        "segment_cache_evictions",
        s.segment_cache_evictions
    );
    field_hwm!(&mut buf, HWM_ROPE_BYTES, s.rope_bytes);
    field_hwm!(
        &mut buf,
        HWM_SNAPSHOT_HISTORY_BYTES,
        s.snapshot_history_bytes
    );
    field_hwm!(
        &mut buf,
        HWM_DECORATION_CACHE_BYTES,
        s.decoration_cache_bytes
    );
    field_hwm!(
        &mut buf,
        HWM_DECORATION_CACHE_ENTRIES,
        s.decoration_cache_entries
    );
    field_plain!(&mut buf, "decoration_cache_hits", s.decoration_cache_hits);
    field_plain!(
        &mut buf,
        "decoration_cache_misses",
        s.decoration_cache_misses
    );
    field_plain!(
        &mut buf,
        "decoration_cache_evictions",
        s.decoration_cache_evictions
    );
    field_hwm!(&mut buf, HWM_TREE_CACHE_BYTES, s.tree_cache_bytes);
    field_hwm!(
        &mut buf,
        HWM_TREE_SITTER_HEAP_BYTES,
        s.tree_sitter_heap_bytes
    );
    field_hwm!(&mut buf, HWM_UNDO_TREE_BYTES, s.undo_tree_bytes);
    field_hwm!(&mut buf, HWM_UNDO_TREE_RECORDS, s.undo_tree_records);
    field_hwm!(&mut buf, HWM_UNDO_TREE_GROUPS, s.undo_tree_groups);
    field_hwm!(&mut buf, HWM_IMAGE_CACHE_BYTES, s.image_cache_bytes);
    field_hwm!(
        &mut buf,
        HWM_PERSIST_UNFLUSHED_BYTES,
        s.persist_unflushed_bytes
    );
    field_hwm!(
        &mut buf,
        HWM_PROJECTION_QUEUE_DEPTH,
        s.projection_queue_depth
    );
    field_plain!(
        &mut buf,
        "projection_queue_capacity",
        s.projection_queue_capacity
    );
    field_hwm!(&mut buf, HWM_ROPE_GENERATIONS_LIVE, s.rope_generations_live);
    field_hwm!(&mut buf, HWM_ROPE_SNAPSHOTS_LIVE, s.rope_snapshots_live);
    // Graphics (DirectWrite / D3D / GPU). GPU figures are mostly NOT
    // part of `private_bytes`; the report keeps them separate from the
    // residual's sum-of-known.
    field_hwm!(
        &mut buf,
        HWM_DWRITE_OWNED_CACHE_BYTES,
        s.dwrite_owned_cache_bytes
    );
    field_hwm!(&mut buf, HWM_GPU_LOCAL_BYTES, s.gpu_local_bytes);
    field_hwm!(&mut buf, HWM_GPU_NONLOCAL_BYTES, s.gpu_nonlocal_bytes);
    field_hwm!(&mut buf, HWM_SWAPCHAIN_BYTES, s.swapchain_bytes);
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hwm_only_advances_on_higher_values() {
        let hwm = Hwm::new("test");
        assert_eq!(hwm.observe(10), 10);
        assert_eq!(hwm.observe(5), 10);
        assert_eq!(hwm.observe(25), 25);
        assert_eq!(hwm.observe(25), 25);
    }
}
