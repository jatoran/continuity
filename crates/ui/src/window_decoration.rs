//! Phase-10 plumbing: per-window decoration worker pool, revision-keyed
//! decoration cache, and per-buffer language detection.
//!
//! **Thread ownership**:
//! - `DecoratePool` owns its worker threads; the [`Window`] holds the only
//!   handle to it on the UI thread.
//! - The `DecorationCache` lives on the UI thread alongside the
//!   `LayoutCache`; only the UI thread reads or writes it.
//! - The `Language` and `language_revision` fields are UI-thread state
//!   recomputed on demand against the current `RopeSnapshot`.

use continuity_core::EditorSnapshot;
use continuity_decorate::{
    detect, DecoratePool, DecorateRequest, DecorateResult, DecorationFullParseReason,
    DecorationParseTrace, Decorations, Language,
};
use std::time::Duration;

use crate::Window;

/// How many decoration worker threads to spawn per window.
///
/// Two is enough for a single-buffer Phase-10 baseline (one for the active
/// buffer plus headroom). Phase 13's pane tree may want to size this with
/// pane count.
pub(crate) const DECORATE_WORKERS: usize = 2;
/// Bound for the worker → UI result channel. Workers are fast enough to
/// keep this small in practice.
pub(crate) const DECORATE_RESULT_CAPACITY: usize = 8;

fn convert_edit_point(point: continuity_core::EditPoint) -> continuity_decorate::EditPoint {
    continuity_decorate::EditPoint::new(point.row, point.column)
}

fn convert_delta_with_points(
    delta: continuity_core::RopeEditDeltaWithPoints,
) -> continuity_decorate::RopeEditDeltaWithPoints {
    continuity_decorate::RopeEditDeltaWithPoints {
        delta: delta.delta,
        start_point: convert_edit_point(delta.start_point),
        old_end_point: convert_edit_point(delta.old_end_point),
        new_end_point: convert_edit_point(delta.new_end_point),
    }
}

fn log_decoration_parse_trace(buffer_id: u128, trace: DecorationParseTrace) {
    if !crate::paint_trace::is_trace_enabled() {
        return;
    }
    match trace {
        DecorationParseTrace::Skipped {
            language,
            elapsed_us,
        } => {
            let detail = format!("buffer={buffer_id} language={language} elapsed_us={elapsed_us}");
            crate::paint_trace::log_event("decoration_parse_skipped", &detail);
        }
        DecorationParseTrace::Incremental {
            delta_count,
            cached_source_len,
            elapsed_us,
            tree_query_us,
            decoration_compute_us,
        } => {
            let detail = format!(
                "buffer={buffer_id} ranges={delta_count} elapsed_us={elapsed_us} \
                 cached_source_len={cached_source_len} \
                 tree_query_us={tree_query_us} decoration_compute_us={decoration_compute_us}"
            );
            crate::paint_trace::log_event("decoration_parse_incremental", &detail);
            crate::paint_trace::log_event(
                "decoration_work",
                &format!(
                    "buffer={buffer_id} path=incremental \
                     tree_query_us={tree_query_us} decoration_compute_us={decoration_compute_us}"
                ),
            );
        }
        DecorationParseTrace::Full {
            reason,
            elapsed_us,
            tree_query_us,
            decoration_compute_us,
        } => {
            let detail = format!(
                "buffer={buffer_id} reason={} elapsed_us={elapsed_us} \
                 tree_query_us={tree_query_us} decoration_compute_us={decoration_compute_us}",
                reason.as_str()
            );
            crate::paint_trace::log_event("decoration_parse_full", &detail);
            crate::paint_trace::log_event(
                "decoration_work",
                &format!(
                    "buffer={buffer_id} path=full \
                     tree_query_us={tree_query_us} decoration_compute_us={decoration_compute_us}"
                ),
            );
        }
    }
}

impl Window {
    fn decoration_delta_payload(
        &self,
        buffer_id: continuity_buffer::BufferId,
        prev_decoration_rev: Option<u64>,
    ) -> (
        Option<u64>,
        std::sync::Arc<[continuity_decorate::RopeEditDeltaWithPoints]>,
        DecorationFullParseReason,
    ) {
        let Some(prev_revision) = prev_decoration_rev else {
            return (
                None,
                continuity_decorate::empty_deltas(),
                DecorationFullParseReason::NoPrevTree,
            );
        };
        let (deltas, covered) = self
            .editor
            .rope_deltas_with_points_since(buffer_id, prev_revision);
        if !covered {
            return (
                None,
                continuity_decorate::empty_deltas(),
                DecorationFullParseReason::CoveredFalse,
            );
        }
        let converted: Vec<continuity_decorate::RopeEditDeltaWithPoints> =
            deltas.into_iter().map(convert_delta_with_points).collect();
        (
            Some(prev_revision),
            std::sync::Arc::from(converted.into_boxed_slice()),
            DecorationFullParseReason::NoPrevTree,
        )
    }

    /// Submit a fresh snapshot to the decoration pool whenever the active
    /// buffer's revision has advanced past the last submitted one. No-op
    /// when there's no active snapshot or the pool hasn't been spawned.
    pub(crate) fn maybe_submit_decoration(&mut self) {
        let Some(pool) = self.decorate_pool.as_ref() else {
            return;
        };
        let Some(snap) = self.editor.snapshot(self.buffer_id) else {
            return;
        };
        let rev = snap.rope_snapshot().revision().get();
        let buffer_id = self.buffer_id.as_uuid().as_u128();
        if let Some(last) = self.last_submitted_decoration_revision {
            if last >= rev {
                return;
            }
        }
        // P17.1 — ship the rope as `Arc<Rope>`. The worker materializes
        // to a flat `String` only after the latest-wins queue picks
        // this request as the survivor, so a typing storm pays one
        // `rope.to_string()` instead of one per keystroke.
        let prev_decoration_rev = self
            .decoration_cache
            .get(buffer_id)
            .map(|decorations| decorations.revision);
        let (prev_revision, deltas_since_prev, full_parse_reason) =
            self.decoration_delta_payload(self.buffer_id, prev_decoration_rev);
        let rope = std::sync::Arc::clone(snap.rope_snapshot().rope_arc());
        let language = detect_language_for_snapshot(&snap);
        self.language = language;
        self.language_revision = Some(rev);
        let _ = pool.request(DecorateRequest {
            buffer_id,
            revision: rev,
            rope,
            language,
            prev_revision,
            deltas_since_prev,
            full_parse_reason,
        });
        self.last_submitted_decoration_revision = Some(rev);
        self.last_submitted_decoration_revision_per_buffer
            .borrow_mut()
            .insert(self.buffer_id, rev);
    }

    /// Submit decoration requests for every spectator pane's buffer when
    /// the cache's snapshot is stale (or missing). Called from the paint
    /// path so a buffer that has never been focused since startup still
    /// renders with markdown styling once the worker pool catches up.
    ///
    /// Dedupes by buffer id (a buffer may be open in multiple panes) and
    /// skips the focused buffer (handled by
    /// [`Window::maybe_submit_decoration`]). The submission is gated on
    /// the cache's stored revision, so workers don't get re-flooded when
    /// nothing has changed; a brief duplicate while a request is in
    /// flight is acceptable — the pool's bounded result channel and the
    /// cache's revision-monotonic insert dedupe the outcome.
    pub(crate) fn submit_decorations_for_visible_panes(&mut self) {
        let Some(pool) = self.decorate_pool.as_ref() else {
            return;
        };
        let focused_id = self.buffer_id.as_uuid().as_u128();
        let mut seen: std::collections::HashSet<u128> = std::collections::HashSet::new();
        seen.insert(focused_id);
        // Snapshot the leaf list up-front so we don't hold a borrow into
        // `self.tree` while we mutate via `editor.snapshot` / channel
        // sends below.
        let leaves: Vec<crate::pane_tree::PaneId> = self
            .tree
            .root
            .leaf_ids()
            .into_iter()
            .filter(|pid| *pid != self.tree.focused)
            .collect();
        for pane_id in leaves {
            let Some(group) = self.tree.groups.get(&pane_id) else {
                continue;
            };
            let Some(active_tab) = self.tree.tabs.get(&group.active) else {
                continue;
            };
            let buffer_id = active_tab.buffer_id;
            let buffer_id_u128 = buffer_id.as_uuid().as_u128();
            if !seen.insert(buffer_id_u128) {
                continue;
            }
            let Some(snap) = self.editor.snapshot(buffer_id) else {
                continue;
            };
            let rev = snap.rope_snapshot().revision().get();
            let cache_rev = self
                .decoration_cache
                .get(buffer_id_u128)
                .map(|d| d.revision);
            if cache_rev == Some(rev) {
                continue;
            }
            if self
                .last_submitted_decoration_revision_per_buffer
                .borrow()
                .get(&buffer_id)
                .is_some_and(|last| *last >= rev)
            {
                continue;
            }
            let (prev_revision, deltas_since_prev, full_parse_reason) =
                self.decoration_delta_payload(buffer_id, cache_rev);
            let rope = std::sync::Arc::clone(snap.rope_snapshot().rope_arc());
            let language = detect_language_for_snapshot(&snap);
            if pool
                .request(DecorateRequest {
                    buffer_id: buffer_id_u128,
                    revision: rev,
                    rope,
                    language,
                    prev_revision,
                    deltas_since_prev,
                    full_parse_reason,
                })
                .is_ok()
            {
                self.last_submitted_decoration_revision_per_buffer
                    .borrow_mut()
                    .insert(buffer_id, rev);
            }
        }
    }

    /// Submit a decoration request for any live buffer. Used by idle
    /// display-map prewarm so MRU-adjacent tabs can reach the decorated
    /// stage without becoming visible first. The worker pool keeps only
    /// the latest pending request per buffer, so repeated idle ticks
    /// coalesce naturally.
    pub(crate) fn submit_decoration_for_buffer(&self, buffer_id: continuity_buffer::BufferId) {
        let Some(pool) = self.decorate_pool.as_ref() else {
            return;
        };
        let Some(snap) = self.editor.snapshot(buffer_id) else {
            return;
        };
        let revision = snap.rope_snapshot().revision().get();
        let document = buffer_id.as_uuid().as_u128();
        if self
            .decoration_cache
            .get(document)
            .is_some_and(|decorations| decorations.revision >= revision)
        {
            return;
        }
        let prev_decoration_rev = self
            .decoration_cache
            .get(document)
            .map(|decorations| decorations.revision);
        let (prev_revision, deltas_since_prev, full_parse_reason) =
            self.decoration_delta_payload(buffer_id, prev_decoration_rev);
        let rope = std::sync::Arc::clone(snap.rope_snapshot().rope_arc());
        let language = detect_language_for_snapshot(&snap);
        if pool
            .request(DecorateRequest {
                buffer_id: document,
                revision,
                rope,
                language,
                prev_revision,
                deltas_since_prev,
                full_parse_reason,
            })
            .is_ok()
        {
            self.last_submitted_decoration_revision_per_buffer
                .borrow_mut()
                .insert(buffer_id, revision);
        }
    }

    /// Used to recompute decorations synchronously on the UI thread
    /// when the worker pool was behind, to mask the round-trip and
    /// avoid a one-frame "markers reappear while typing" flicker.
    ///
    /// **Now a no-op.** The line-local caret-reveal rule in
    /// `crates/display_map/src/builder/segments.rs::line_revealed`
    /// makes the typing-flicker invisible regardless of decoration
    /// staleness — reveal state depends only on rope-line bounds vs
    /// the current caret position, never on stale block byte
    /// ranges. Without that dependency the sync parse was paying a
    /// 50–80 ms UI-thread block per keystroke on a multi-thousand-
    /// line document for no visible benefit; trace inspection
    /// surfaced this as the dominant per-keystroke cost. Decorations
    /// continue to update through the worker pool's async path; the
    /// `Decorations::compute` call is preserved in source so a
    /// future regression that needs sync masking can reach for it.
    pub(crate) fn ensure_fresh_decorations(&mut self, snap: &EditorSnapshot) {
        let _ = snap;
        let _ = Decorations::compute;
    }

    /// Drain any ready decoration results from the pool's receiver and
    /// merge them into the per-buffer cache. Stale results (revision below
    /// the buffer's current revision) are still cached — the cache itself
    /// rejects regressions.
    pub(crate) fn drain_decoration_results(&mut self) -> bool {
        let _ = self.drain_decoration_watchdog_events(self.now_ms());
        let mut updated_buffer_ids: Vec<u128> = Vec::new();
        // Scope the pool borrow so the spectator-prewarm dispatch
        // below can take `&mut self` without fighting the borrow
        // checker. The pool only needs to stay alive long enough to
        // drain its result channel into `updated_buffer_ids`.
        {
            let Some(pool) = self.decorate_pool.as_ref() else {
                return false;
            };
            while let Ok(DecorateResult {
                buffer_id,
                language,
                outcome,
                parse_trace,
            }) = pool.results().try_recv()
            {
                log_decoration_parse_trace(buffer_id, parse_trace);
                match outcome {
                    Ok(d) => {
                        let revision = d.revision;
                        let resolved_buffer_id = continuity_buffer::BufferId::from_uuid(
                            uuid::Uuid::from_u128(buffer_id),
                        );
                        let Some(snap) = self.editor.snapshot(resolved_buffer_id) else {
                            continue;
                        };
                        if detect_language_for_snapshot(&snap) != language {
                            continue;
                        }
                        self.last_submitted_decoration_revision_per_buffer
                            .borrow_mut()
                            .insert(resolved_buffer_id, revision);
                        if self.decoration_cache.insert(buffer_id, d) {
                            self.display_map_prewarm
                                .invalidate_decoration_revision(buffer_id, revision);
                            if !updated_buffer_ids.contains(&buffer_id) {
                                updated_buffer_ids.push(buffer_id);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("decoration worker error: {e}");
                    }
                }
            }
        }
        let updated = !updated_buffer_ids.is_empty();
        if updated {
            self.try_dispatch_decoration_change_projection_worker_for_live_panes(
                &updated_buffer_ids,
            );
        }
        updated
    }

    /// Refresh the cached language identifier for the active buffer.
    /// Cheap: O(first-16-non-empty-lines) when no path is associated.
    pub(crate) fn refresh_language(&mut self) {
        let Some(snap) = self.editor.snapshot(self.buffer_id) else {
            return;
        };
        let rev = snap.rope_snapshot().revision().get();
        // Skip recompute if revision unchanged.
        if let Some(last) = self.language_revision {
            if last == rev {
                return;
            }
        }
        self.language = detect_language_for_snapshot(&snap);
        self.language_revision = Some(rev);
    }

    /// Stable string for the predicate grammar's `language` atom.
    #[must_use]
    pub(crate) fn language_atom(&self) -> &'static str {
        self.language.as_str()
    }

    /// Initialize the decorate pool. Called once during window construction.
    /// Storing `None` until ready means tests that build a `Window` without
    /// the full pipeline still operate.
    pub(crate) fn install_decorate_pool(&mut self) {
        if self.decorate_pool.is_none() {
            self.decorate_pool = Some(DecoratePool::spawn_with_watchdog_timeout(
                DECORATE_WORKERS,
                DECORATE_RESULT_CAPACITY,
                Duration::from_millis(u64::from(self.decoration_worker_watchdog_timeout_ms)),
            ));
        }
    }

    /// Default language for a freshly-opened buffer with no path/content.
    #[must_use]
    pub(crate) fn default_language() -> Language {
        Language::Plain
    }
}

fn detect_language_for_snapshot(snap: &EditorSnapshot) -> Language {
    let rope = snap.rope_snapshot().rope();
    let head_chars: usize = rope.len_chars().min(2048);
    let head: String = rope.slice(0..head_chars).chars().collect();
    // Phase 15 wired file IO: when the buffer is associated with a path,
    // its extension is the authoritative language hint and beats the
    // content sniff. Untitled buffers default to markdown per spec §3.
    let ext = snap
        .file
        .as_ref()
        .and_then(|f| f.path.extension())
        .and_then(|e| e.to_str())
        .map(str::to_owned);
    detect(ext.as_deref(), &head)
}
