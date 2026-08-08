//! Shared fixtures for `DisplayMapBuilder` parity tests across the
//! integration-test binaries in `crates/display_map/tests/`.
//!
//! Cargo compiles each `tests/*.rs` file as its own binary; this
//! module is the workspace's replacement for the conventional
//! `tests/common/mod.rs` pattern (forbidden by the no-`mod.rs`
//! convention).

use std::sync::Arc;

use continuity_buffer::{Revision, RopeSnapshot};
use continuity_decorate::Decorations;
use ropey::Rope;

use crate::wrap::FixedCharWidth;
use crate::{DisplayLine, DisplayMap, DisplayMapBuilder, RowSplice, SourceLine, WrapConfig};

/// Build a viewport `DisplayMap` from raw text + a fixed-width measurer.
pub fn build_viewport(
    text: &str,
    wrap_width: u32,
    visible: std::ops::Range<u32>,
    overscan: u32,
) -> Arc<DisplayMap> {
    let snap = RopeSnapshot::new(Arc::new(Rope::from_str(text)), Revision(1));
    let decos = Decorations::empty(1);
    let mut measure = FixedCharWidth::new(8.0);
    let wrap = if wrap_width == 0 {
        WrapConfig::NONE
    } else {
        WrapConfig::new(wrap_width)
    };
    DisplayMapBuilder::new(&snap, &decos, &[], &[], wrap)
        .build_viewport(visible, overscan, &mut measure)
        .expect("viewport build ok")
}

/// Rebuild a viewport via the dirty-set path against a previous map.
pub fn rebuild_dirty(
    text: &str,
    wrap_width: u32,
    visible: std::ops::Range<u32>,
    overscan: u32,
    prev: &DisplayMap,
    dirty: &[u32],
    revision: u64,
) -> Arc<DisplayMap> {
    let snap = RopeSnapshot::new(Arc::new(Rope::from_str(text)), Revision(revision));
    let decos = Decorations::empty(revision);
    let mut measure = FixedCharWidth::new(8.0);
    let wrap = if wrap_width == 0 {
        WrapConfig::NONE
    } else {
        WrapConfig::new(wrap_width)
    };
    DisplayMapBuilder::new(&snap, &decos, &[], &[], wrap)
        .rebuild_dirty(prev, dirty, visible, overscan, &mut measure)
        .expect("rebuild_dirty ok")
}

/// Rebuild a viewport via the row-splice path against a previous map.
pub fn rebuild_spliced(
    text: &str,
    wrap_width: u32,
    visible: std::ops::Range<u32>,
    overscan: u32,
    prev: &DisplayMap,
    splice: &RowSplice,
    revision: u64,
) -> Arc<DisplayMap> {
    let snap = RopeSnapshot::new(Arc::new(Rope::from_str(text)), Revision(revision));
    let decos = Decorations::empty(revision);
    let mut measure = FixedCharWidth::new(8.0);
    let wrap = if wrap_width == 0 {
        WrapConfig::NONE
    } else {
        WrapConfig::new(wrap_width)
    };
    DisplayMapBuilder::new(&snap, &decos, &[], &[], wrap)
        .rebuild_spliced(prev, splice, visible, overscan, &mut measure)
        .expect("rebuild_spliced ok")
}

/// Assert two maps describe the same display geometry for the realized
/// row range — row counts, source-line counts, per-line row counts, and
/// the realized text / source-byte spans for every realized row.
pub fn assert_maps_equivalent(reference: &DisplayMap, candidate: &DisplayMap) {
    assert_eq!(
        candidate.row_index().display_row_count(),
        reference.row_index().display_row_count(),
        "display row count must match",
    );
    assert_eq!(
        candidate.row_index().source_line_count(),
        reference.row_index().source_line_count(),
        "source line count must match",
    );
    for i in 0..reference.row_index().source_line_count() {
        let line = SourceLine(i);
        assert_eq!(
            candidate.row_index().display_row_count_for_source(line),
            reference.row_index().display_row_count_for_source(line),
            "row count mismatch at source_line={i}",
        );
    }
    let r_realized = reference.realized_row_range();
    let c_realized = candidate.realized_row_range();
    assert_eq!(
        c_realized, r_realized,
        "realized row range must match the from-scratch viewport build",
    );
    for absolute_row in r_realized.start..r_realized.end {
        let r_spec = reference
            .display_line(DisplayLine(absolute_row))
            .expect("invariant: row inside realized range resolves on reference");
        let c_spec = candidate
            .display_line(DisplayLine(absolute_row))
            .expect("invariant: row inside realized range resolves on candidate");
        assert_eq!(
            c_spec.display_text(),
            r_spec.display_text(),
            "display text mismatch at row {absolute_row}",
        );
        assert_eq!(
            c_spec.source_line.raw(),
            r_spec.source_line.raw(),
            "source line mismatch at row {absolute_row}",
        );
        assert_eq!(
            c_spec.source_byte_start.raw(),
            r_spec.source_byte_start.raw(),
            "source_byte_start mismatch at row {absolute_row}",
        );
        assert_eq!(
            c_spec.source_byte_end.raw(),
            r_spec.source_byte_end.raw(),
            "source_byte_end mismatch at row {absolute_row}",
        );
    }
}
