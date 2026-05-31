//! Cold-deferred stub eligibility for the focused-pane Cold path.
//!
//! Extracted from `window_paint/frame_resolution.rs` so that file
//! stays under the conventions cap. The single responsibility here is
//! deciding whether a candidate [`FrameDisplay`] (a same-buffer cached
//! frame at the current `wrap_width_dip`) can substitute for an
//! inline row-count walker when [`crate::window_projection_plan::classify_projection_build`]
//! returned `Cold` and the projection worker has not delivered the
//! real frame yet.
//!
//! Document match is enforced **upstream** by the call site (paint's
//! `last_painted_frame_display` pairs the query with the frame, and
//! `crate::window_spectator_cache::SpectatorFrameCache::lookup_same_document`
//! performs the check internally). Index stamps do not carry the
//! buffer id, so a wrong-document candidate that happened to share a
//! rope-revision counter would otherwise slip past the checks below.
//!
//! Thread ownership: UI thread of one window. Pure function over a
//! caller-cloned candidate frame.

use continuity_render::FrameDisplay;

/// Source-line threshold below which the focused-pane Cold path
/// always cold-builds inline instead of substituting a candidate frame.
/// On a short buffer the walker is fast enough that a substitution is
/// not worth the policy complexity. Mirror the spectator stub's
/// 2 000-line threshold so both paths kick in for the same "large
/// buffer" class.
pub(crate) const COLD_DEFERRED_STUB_LINE_THRESHOLD: usize = 2_000;

/// Why a Cold-paint substitution was *not* performed. The trace
/// consumer keys `paint:frame_display:cold_deferred_skip reason=…`
/// off these stable spellings.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum ColdDeferredSkip {
    /// No candidate frame at all — neither `last_painted_frame_display`
    /// nor the spectator cache held a same-document entry. First-paint
    /// after app start, or buffer adopted into a pane that has never
    /// painted it.
    NoCandidate,
    /// Buffer is below [`COLD_DEFERRED_STUB_LINE_THRESHOLD`] source
    /// lines; the walker is fast enough that substitution is not worth
    /// the extra path. Inline cold-build runs.
    BufferTooSmall,
    /// Candidate's `wrap_width_dip` differs from the current paint's.
    /// Substituting would visibly wrap text at the wrong width.
    WrapWidthMismatch,
    /// Candidate's `rope_revision` differs from the current paint's
    /// — substituting would paint stale text. Typing burst plus a
    /// buffer-switch is the common cause.
    RopeRevisionDrift,
    /// Candidate's `decoration_revision` differs from the current
    /// paint's — substituting would paint stale styling.
    DecorationRevisionDrift,
}

impl ColdDeferredSkip {
    /// Stable trace spelling matched by perf scripts.
    #[must_use]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NoCandidate => "no_candidate",
            Self::BufferTooSmall => "buffer_too_small",
            Self::WrapWidthMismatch => "wrap_width_mismatch",
            Self::RopeRevisionDrift => "rope_revision_drift",
            Self::DecorationRevisionDrift => "decoration_revision_drift",
        }
    }
}

/// Stub eligibility for a Cold build. Returns the candidate frame
/// when:
///
/// - `stamps.rope_revision == rope_revision` (painted text matches
///   current rope content byte-for-byte; rope drift means stale
///   text which is not acceptable for the focused pane).
/// - `stamps.decoration_revision == current_decoration_rev.unwrap_or(rope_revision)`
///   (`IndexStamps::decoration_revision` mirrors `Decorations::revision`,
///   `None` decorations fall back to the rope revision per
///   `Decorations::empty(rev)`).
/// - `stamps.wrap_width_dip == wrap_width_dip`; a different-wrap frame
///   is stale geometry and would visibly wrap text at the wrong width.
///
/// The walker dominates Cold builds (~54 µs per source line × 9 016
/// lines = ~480 ms on a 9 k-line markdown buffer — see
/// `perf-snapshots/manual-lag_after-coalesce_20260518-164726.tsv`
/// `frame_display:cold_build dur=448572`). Substituting is only valid
/// when the cached frame already matches the current wrap geometry.
pub(crate) fn cold_deferred_stub_frame(
    candidate: Option<FrameDisplay>,
    rope_revision: u64,
    current_decoration_rev: Option<u64>,
    wrap_width_dip: u32,
    source_line_count: usize,
) -> Result<FrameDisplay, ColdDeferredSkip> {
    if source_line_count < COLD_DEFERRED_STUB_LINE_THRESHOLD {
        return Err(ColdDeferredSkip::BufferTooSmall);
    }
    let candidate = candidate.ok_or(ColdDeferredSkip::NoCandidate)?;
    let stamps = candidate.row_index().stamps();
    let candidate_decoration_rev = stamps.decoration_revision;
    let expected_decoration_rev = current_decoration_rev.unwrap_or(rope_revision);
    if stamps.rope_revision != rope_revision {
        return Err(ColdDeferredSkip::RopeRevisionDrift);
    }
    if candidate_decoration_rev != expected_decoration_rev {
        return Err(ColdDeferredSkip::DecorationRevisionDrift);
    }
    if stamps.wrap_width_dip != wrap_width_dip {
        return Err(ColdDeferredSkip::WrapWidthMismatch);
    }
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;

    fn frame_at(rope_rev: u64, wrap: u32) -> FrameDisplay {
        let rope = Rope::from_str("same\n");
        FrameDisplay::build(&rope, rope_rev, None, &[0], wrap, 8.0)
    }

    /// Use a line count above the threshold so the size gate doesn't
    /// shadow the field under test.
    const LARGE: usize = COLD_DEFERRED_STUB_LINE_THRESHOLD + 1;

    #[test]
    fn cold_deferred_stub_requires_same_rope_revision() {
        let cached = frame_at(1, 480);
        assert!(cold_deferred_stub_frame(Some(cached.clone()), 1, None, 480, LARGE).is_ok());
        assert_eq!(
            cold_deferred_stub_frame(Some(cached), 2, None, 480, LARGE).err(),
            Some(ColdDeferredSkip::RopeRevisionDrift),
        );
    }

    #[test]
    fn cold_deferred_stub_rejects_wrap_width_change() {
        let cached = frame_at(7, 480);
        // Same wrap is safe to substitute.
        assert!(cold_deferred_stub_frame(Some(cached.clone()), 7, None, 480, LARGE).is_ok());
        // Different wrap would visibly paint the wrong geometry.
        assert_eq!(
            cold_deferred_stub_frame(Some(cached), 7, None, 640, LARGE).err(),
            Some(ColdDeferredSkip::WrapWidthMismatch),
        );
    }

    #[test]
    fn cold_deferred_stub_rejects_decoration_drift() {
        // Cached frame built undecorated (decoration_revision ==
        // rope_revision == 5).
        let cached = frame_at(5, 480);
        // Current paint has decorations at rev 9; helper should reject
        // because the painted text would carry stale styling.
        assert_eq!(
            cold_deferred_stub_frame(Some(cached), 5, Some(9), 480, LARGE).err(),
            Some(ColdDeferredSkip::DecorationRevisionDrift),
        );
    }

    #[test]
    fn cold_deferred_stub_returns_none_when_no_candidate() {
        assert_eq!(
            cold_deferred_stub_frame(None, 1, None, 240, LARGE).err(),
            Some(ColdDeferredSkip::NoCandidate),
        );
    }

    #[test]
    fn cold_deferred_stub_skips_small_buffer_first() {
        // Even with a perfectly eligible candidate, a buffer below
        // the threshold falls through to the inline cold-build to
        // avoid the visible wrap shift on a paint the walker would
        // have completed sub-ms anyway.
        let cached = frame_at(1, 480);
        assert_eq!(
            cold_deferred_stub_frame(
                Some(cached),
                1,
                None,
                480,
                COLD_DEFERRED_STUB_LINE_THRESHOLD - 1,
            )
            .err(),
            Some(ColdDeferredSkip::BufferTooSmall),
        );
    }

    #[test]
    fn skip_reason_strings_are_stable() {
        assert_eq!(ColdDeferredSkip::NoCandidate.as_str(), "no_candidate");
        assert_eq!(
            ColdDeferredSkip::BufferTooSmall.as_str(),
            "buffer_too_small"
        );
        assert_eq!(
            ColdDeferredSkip::WrapWidthMismatch.as_str(),
            "wrap_width_mismatch"
        );
        assert_eq!(
            ColdDeferredSkip::RopeRevisionDrift.as_str(),
            "rope_revision_drift"
        );
        assert_eq!(
            ColdDeferredSkip::DecorationRevisionDrift.as_str(),
            "decoration_revision_drift"
        );
    }
}
