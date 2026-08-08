//! Caller-thread-owned incremental Markdown projection cache.

use continuity_buffer::RopeSnapshot;
use continuity_decorate::{
    CachedBufferTree, Decorations, EditPoint, RopeEditDeltaWithPoints as DecorationDelta,
};
use continuity_engine::RopeEditDeltaWithPoints as EngineDelta; // alias: collides with continuity_decorate::RopeEditDeltaWithPoints

use crate::projection::{compute_range_report_with_decorations, compute_report_with_decorations};

/// Cached parse tree, decorations, and serialized reports for one WASM editor.
#[derive(Default)]
pub(crate) struct ProjectionCache {
    decorations: Option<Decorations>,
    full_json: Option<String>,
    presentation_json: Option<String>,
    tree: Option<CachedBufferTree>,
}

impl ProjectionCache {
    pub(crate) fn revision(&self) -> Option<u64> {
        self.tree.as_ref().map(|tree| tree.revision)
    }

    pub(crate) fn report_json(
        &mut self,
        snapshot: &RopeSnapshot,
        deltas: &[EngineDelta],
        is_delta_history_covered: bool,
        should_include_mappings: bool,
    ) -> Result<String, String> {
        self.update(snapshot, deltas, is_delta_history_covered)?;
        let cached = if should_include_mappings {
            &mut self.full_json
        } else {
            &mut self.presentation_json
        };
        if let Some(json) = cached {
            return Ok(json.clone());
        }
        let decorations = self
            .decorations
            .as_ref()
            .ok_or_else(|| "Markdown decorations are unavailable".to_string())?;
        let report =
            compute_report_with_decorations(snapshot, decorations, should_include_mappings)?;
        let json = serde_json::to_string(&report).map_err(|error| error.to_string())?;
        *cached = Some(json.clone());
        Ok(json)
    }

    pub(crate) fn range_report_json(
        &mut self,
        snapshot: &RopeSnapshot,
        deltas: &[EngineDelta],
        is_delta_history_covered: bool,
        start_line: u32,
        end_line: u32,
    ) -> Result<String, String> {
        self.update(snapshot, deltas, is_delta_history_covered)?;
        let decorations = self
            .decorations
            .as_ref()
            .ok_or_else(|| "Markdown decorations are unavailable".to_string())?;
        let report =
            compute_range_report_with_decorations(snapshot, decorations, start_line, end_line)?;
        serde_json::to_string(&report).map_err(|error| error.to_string())
    }

    fn update(
        &mut self,
        snapshot: &RopeSnapshot,
        deltas: &[EngineDelta],
        is_delta_history_covered: bool,
    ) -> Result<(), String> {
        let revision = snapshot.revision().get();
        if self.revision() == Some(revision) {
            return Ok(());
        }
        let source = snapshot.rope().to_string();
        let decoration_deltas = deltas.iter().map(convert_delta).collect::<Vec<_>>();
        let computed = self.tree.as_ref().and_then(|previous| {
            is_delta_history_covered.then(|| {
                Decorations::compute_incremental(
                    &source,
                    revision,
                    &previous.tree,
                    &decoration_deltas,
                    previous.source_len,
                )
            })?
        });
        let (decorations, tree) = computed
            .or_else(|| Decorations::compute_with_tree(&source, revision))
            .ok_or_else(|| "Markdown parser did not return a tree".to_string())?;
        self.tree = Some(CachedBufferTree {
            revision,
            source_len: source.len(),
            tree,
        });
        self.decorations = Some(decorations);
        self.full_json = None;
        self.presentation_json = None;
        Ok(())
    }
}

fn convert_delta(delta: &EngineDelta) -> DecorationDelta {
    DecorationDelta {
        delta: delta.delta,
        start_point: EditPoint::new(delta.start_point.row, delta.start_point.column),
        old_end_point: EditPoint::new(delta.old_end_point.row, delta.old_end_point.column),
        new_end_point: EditPoint::new(delta.new_end_point.row, delta.new_end_point.column),
    }
}
