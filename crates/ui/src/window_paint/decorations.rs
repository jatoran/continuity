//! Decorations resolution for `Window::on_paint` — picks the snapshot
//! to paint with, transforming stale byte ranges through the rope edits
//! that have landed since the worker computed them.
//!
//! Stale decorations are reused (with byte ranges shifted forward
//! through `transformed_through`) rather than dropped, so markdown
//! styling does not blink to plain-text every time the decoration
//! worker falls a keystroke behind on a large file. The `Arc` wrap is
//! built exactly once per paint and shared by the paint-cache
//! writeback and the worker request submission.

use std::sync::Arc;

use continuity_decorate::Decorations;

use crate::window::Window;

/// Output of [`Window::resolve_decorations_for_paint`].
pub(crate) struct ResolvedDecorations {
    /// Decorations snapshot the paint should consume.
    pub owned: Option<Arc<Decorations>>,
    /// Parse revision of the cache entry consumed this paint, captured
    /// **before** any `transformed_through` rewrites the revision label.
    pub current_parse_revision: Option<u64>,
    /// Whether `current_parse_revision` differs from the prior paint's.
    pub parse_advanced: bool,
}

impl Window {
    pub(crate) fn resolve_decorations_for_paint(
        &self,
        decoration_id: u128,
        revision_for_projection: u64,
    ) -> ResolvedDecorations {
        // Snapshot the **worker's parse revision** BEFORE the
        // `transformed_through` branch below rewrites
        // `Decorations::revision` to the current rope rev. This is the
        // ground-truth identifier of the parse content (the rope rev
        // the worker submitted at). Carried into the classifier so a
        // stale parse vs. a fresh parse with the same transformed
        // rope-rev label can be distinguished — see the
        // `Window::last_painted_decoration_parse_revision` doc for the
        // bug-class this closes.
        let current_parse_revision: Option<u64> =
            self.decoration_cache.get(decoration_id).map(|d| d.revision);
        let parse_advanced = current_parse_revision != self.last_painted_decoration_parse_revision;
        if crate::paint_trace::is_trace_enabled() {
            crate::paint_trace::log_event(
                "decoration_parse_revision",
                &format!(
                    "current={current_parse_revision:?} prev={:?} advanced={parse_advanced}",
                    self.last_painted_decoration_parse_revision,
                ),
            );
        }
        // ε.5b: wrap the owned `Decorations` in `Arc` once here so
        // both the worker request submission and
        // `last_painted_decorations` reuse the same allocation.
        // Previously these two paths each did their own deep `clone()`
        // — converting to `Arc` keeps the per-paint cost at the one
        // unavoidable `.cloned()` against the decoration cache.
        let owned: Option<Arc<Decorations>> =
            match self.decoration_cache.get_arc(decoration_id).cloned() {
                None => {
                    crate::paint_trace::log_event("decorations_state", "miss=cache_empty");
                    None
                }
                Some(d) if d.revision == revision_for_projection => {
                    crate::paint_trace::log_event("decorations_state", "exact");
                    Some(d)
                }
                Some(d) => {
                    let (deltas, covered) =
                        self.editor.rope_deltas_since(self.buffer_id, d.revision);
                    if !covered {
                        crate::paint_trace::log_event(
                            "decorations_state",
                            "dropped=history_overflowed",
                        );
                        None
                    } else if deltas.is_empty() {
                        // No deltas between cached and current revision —
                        // the rope is at the cached revision, just not
                        // labelled as such (rare; defensive).
                        crate::paint_trace::log_event("decorations_state", "exact_no_deltas");
                        Some(d)
                    } else {
                        crate::paint_trace::log_event(
                            "decorations_state",
                            &format!(
                                "transformed cached_rev={} rope_rev={} deltas={}",
                                d.revision,
                                revision_for_projection,
                                deltas.len(),
                            ),
                        );
                        Some(Arc::new(
                            d.transformed_through(&deltas, revision_for_projection),
                        ))
                    }
                }
            };
        ResolvedDecorations {
            owned,
            current_parse_revision,
            parse_advanced,
        }
    }
}
