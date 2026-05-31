//! Phase-I1 time-machine HUD geometry, hit-testing, and tick layout.
//!
//! Pure functions and small POD value types that drive the slider
//! overlay. Pulled out of [`crate::window_time_machine`] so that file
//! stays under the 600-line cap as the HUD's paint integration grows;
//! the geometry and revision↔x-coordinate mapping live here on their
//! own with full unit-test coverage.
//!
//! Thread ownership: pure data structures and pure functions; no
//! ownership at all. Consumers ([`crate::window::Window`]'s WM_PAINT
//! and WM_LBUTTONDOWN / WM_MOUSEMOVE handlers) call into these
//! helpers from the UI thread.
//!
//! What lives here:
//!
//! - [`SliderGeometry`] — positions the strip / thumb / ticks given the
//!   [`SliderPaneRect`] and the buffer's
//!   `(earliest_revision, head_revision)`.
//! - [`SliderTick`] — one tick mark with an `x` coordinate, the
//!   underlying [`continuity_buffer::Revision`], and a kind discriminator
//!   ([`TickKind::NamedSnapshot`] vs. [`TickKind::EditOnlySnapshot`]).
//! - [`SliderHit`] — what a click at `(x, y)` hit (thumb / tick / track
//!   gutter / outside).
//! - [`compute_revision_for_x`] — drag math: clamp x to the strip and
//!   linear-interpolate to a [`continuity_buffer::Revision`] in
//!   `[earliest, head]`.
//! - [`compute_x_for_revision`] — inverse, used to position the thumb
//!   while the user holds Enter or restores from a remembered tick.
//!
//! What does **not** live here (deferred follow-ups, see
//! `.docs/development/wire_I1_time_machine_slider.md`):
//!
//! - The Direct2D paint pass that turns this geometry into pixels.
//! - The mouse-capture / drag-loop wiring inside [`crate::window_mouse`].
//! - The historical-state replay path that materializes the rope at
//!   the previewed revision (currently `ViewOverlay::pinned_revision`
//!   reaches the renderer but no consumer fetches a past snapshot).
//! - The `Enter` content-replace edit and the `Esc` overlay-clear hooks
//!   in [`crate::window_dismiss`].

use continuity_buffer::Revision;
use continuity_persist::SnapshotSummaryRow;

/// Pixel padding inside the slider HUD, in client-rect DIPs.
pub(crate) const HUD_HORIZONTAL_PADDING_DIP: f32 = 24.0;

/// Vertical inset from the bottom of the pane body.
pub(crate) const HUD_BOTTOM_OFFSET_DIP: f32 = 0.0;

/// Total height of the HUD band (background + strip + tick row +
/// labels). Used for hit-testing.
pub(crate) const HUD_BAND_HEIGHT_DIP: f32 = 54.0;

/// Vertical center of the strip inside the HUD band.
pub(crate) const HUD_STRIP_CENTER_OFFSET_DIP: f32 = 22.0;

/// Half-width of the thumb's hit-test rectangle.
pub(crate) const THUMB_HIT_HALF_WIDTH_DIP: f32 = 8.0;

/// Half-width of a tick's hit-test rectangle.
pub(crate) const TICK_HIT_HALF_WIDTH_DIP: f32 = 4.0;

/// Pane body rectangle used to place the slider HUD, in window client
/// DIPs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliderPaneRect {
    /// Left edge of the pane body.
    pub left_dip: f32,
    /// Top edge of the pane body.
    pub top_dip: f32,
    /// Width of the pane body.
    pub width_dip: f32,
    /// Height of the pane body.
    pub height_dip: f32,
}

impl SliderPaneRect {
    fn right_dip(self) -> f32 {
        self.left_dip + self.width_dip.max(0.0)
    }

    fn bottom_dip(self) -> f32 {
        self.top_dip + self.height_dip.max(0.0)
    }
}

/// What kind of revision a tick on the slider points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickKind {
    /// A snapshot row that carries a user-supplied label (e.g.
    /// `"pre-refactor"`, `"draft 1"`). Drawn taller / accented.
    NamedSnapshot,
    /// A snapshot row without a label — every committed
    /// `buffer_snapshots` row that the policy emitted at this
    /// revision. Drawn as the default tick height.
    EditOnlySnapshot,
}

/// One tick on the slider strip.
#[derive(Debug, Clone, PartialEq)]
pub struct SliderTick {
    /// Pixel x-coordinate (DIPs) of the tick on the strip.
    pub x_dip: f32,
    /// The revision the tick points at.
    pub revision: Revision,
    /// Whether the tick has a user-supplied label.
    pub kind: TickKind,
    /// The label text, when [`Self::kind`] is [`TickKind::NamedSnapshot`].
    pub label: Option<String>,
    /// Wall-clock timestamp (unix ms) the snapshot was committed at,
    /// from `buffer_snapshots.created_at`. Used by the hover tooltip.
    pub created_at_ms: i64,
}

/// Geometry of the slider HUD in window client coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct SliderGeometry {
    /// Left edge of the strip in client DIPs.
    pub strip_left_dip: f32,
    /// Right edge of the strip in client DIPs.
    pub strip_right_dip: f32,
    /// Vertical center of the strip in client DIPs.
    pub strip_center_y_dip: f32,
    /// Top edge of the HUD band.
    pub band_top_dip: f32,
    /// Bottom edge of the HUD band.
    pub band_bottom_dip: f32,
    /// Earliest revision the slider spans.
    pub earliest_revision: Revision,
    /// Head revision the slider spans.
    pub head_revision: Revision,
    /// Current preview revision (drives the thumb's x-position). When
    /// the slider first opens this equals [`Self::head_revision`].
    pub preview_revision: Revision,
    /// Resolved tick layout, ascending by `x_dip`.
    pub ticks: Vec<SliderTick>,
}

/// What a mouse hit at `(x, y)` resolves to inside the HUD.
#[derive(Debug, Clone, PartialEq)]
pub enum SliderHit {
    /// The user clicked the draggable thumb. The drag handler should
    /// capture the mouse and follow [`Self::Track`] thereafter.
    Thumb,
    /// The user clicked a labeled or unlabeled tick. The drag handler
    /// should snap to it.
    Tick(SliderTick),
    /// The user clicked somewhere else on the strip — treat the same
    /// as starting a drag on the thumb at the new x-coordinate.
    Track {
        /// The revision the click maps to under
        /// [`compute_revision_for_x`].
        revision: Revision,
    },
    /// Outside the HUD band entirely — the click should fall through.
    Outside,
}

impl SliderGeometry {
    /// Build a geometry for an origin-at-zero pane and revision range.
    ///
    /// `summaries` is the ascending-by-revision list returned by
    /// [`continuity_persist::PersistClient::list_snapshot_summaries`].
    /// Out-of-range revisions are clamped to the strip; an empty
    /// `summaries` produces a strip with no ticks.
    #[must_use]
    pub fn build(
        client_width_dip: f32,
        client_height_dip: f32,
        earliest_revision: Revision,
        head_revision: Revision,
        preview_revision: Revision,
        summaries: &[SnapshotSummaryRow],
    ) -> Self {
        Self::build_in_rect(
            SliderPaneRect {
                left_dip: 0.0,
                top_dip: 0.0,
                width_dip: client_width_dip,
                height_dip: client_height_dip,
            },
            earliest_revision,
            head_revision,
            preview_revision,
            summaries,
        )
    }

    /// Build a geometry for a pane body rect in window client DIPs.
    #[must_use]
    pub fn build_in_rect(
        pane: SliderPaneRect,
        earliest_revision: Revision,
        head_revision: Revision,
        preview_revision: Revision,
        summaries: &[SnapshotSummaryRow],
    ) -> Self {
        let pane_width_dip = pane.width_dip.max(0.0);
        let pane_right_dip = pane.right_dip();
        let pane_bottom_dip = pane.bottom_dip();
        let strip_left_dip = pane.left_dip + HUD_HORIZONTAL_PADDING_DIP.min(pane_width_dip);
        let strip_right_dip = (pane_right_dip - HUD_HORIZONTAL_PADDING_DIP).max(strip_left_dip);
        let band_bottom_dip = (pane_bottom_dip - HUD_BOTTOM_OFFSET_DIP).max(pane.top_dip);
        let band_top_dip = (band_bottom_dip - HUD_BAND_HEIGHT_DIP).max(pane.top_dip);
        let strip_center_y_dip = band_top_dip + HUD_STRIP_CENTER_OFFSET_DIP;
        let ticks = summaries
            .iter()
            .map(|row| {
                let x_dip = compute_x_for_revision(
                    strip_left_dip,
                    strip_right_dip,
                    earliest_revision,
                    head_revision,
                    row.revision,
                );
                let kind = if row.label.is_some() {
                    TickKind::NamedSnapshot
                } else {
                    TickKind::EditOnlySnapshot
                };
                SliderTick {
                    x_dip,
                    revision: row.revision,
                    kind,
                    label: row.label.clone(),
                    created_at_ms: row.created_at_ms,
                }
            })
            .collect();
        Self {
            strip_left_dip,
            strip_right_dip,
            strip_center_y_dip,
            band_top_dip,
            band_bottom_dip,
            earliest_revision,
            head_revision,
            preview_revision,
            ticks,
        }
    }

    /// Hit-test a mouse coordinate against the HUD. Returns
    /// [`SliderHit::Outside`] when `(x, y)` falls outside the band.
    #[must_use]
    pub fn hit_test(&self, x_dip: f32, y_dip: f32) -> SliderHit {
        if y_dip < self.band_top_dip || y_dip > self.band_bottom_dip {
            return SliderHit::Outside;
        }
        if x_dip < self.strip_left_dip - THUMB_HIT_HALF_WIDTH_DIP
            || x_dip > self.strip_right_dip + THUMB_HIT_HALF_WIDTH_DIP
        {
            return SliderHit::Outside;
        }
        let thumb_x = self.thumb_x_dip();
        if (x_dip - thumb_x).abs() <= THUMB_HIT_HALF_WIDTH_DIP {
            return SliderHit::Thumb;
        }
        for tick in &self.ticks {
            if (x_dip - tick.x_dip).abs() <= TICK_HIT_HALF_WIDTH_DIP {
                return SliderHit::Tick(tick.clone());
            }
        }
        let revision = compute_revision_for_x(
            self.strip_left_dip,
            self.strip_right_dip,
            self.earliest_revision,
            self.head_revision,
            x_dip,
        );
        SliderHit::Track { revision }
    }

    /// X-coordinate (DIPs) of the slider thumb for the current
    /// [`Self::preview_revision`].
    #[must_use]
    pub fn thumb_x_dip(&self) -> f32 {
        compute_x_for_revision(
            self.strip_left_dip,
            self.strip_right_dip,
            self.earliest_revision,
            self.head_revision,
            self.preview_revision,
        )
    }

    /// `true` when the strip spans more than one revision (i.e. the
    /// thumb has somewhere to slide to).
    #[must_use]
    pub fn has_drag_range(&self) -> bool {
        self.head_revision > self.earliest_revision
    }
}

/// Map an `x_dip` coordinate inside the slider strip back to a
/// revision in `[earliest, head]`. Coordinates outside the strip are
/// clamped to the nearest endpoint.
///
/// When `earliest >= head` (the buffer has a single revision) every
/// x-coordinate maps to `head`.
#[must_use]
pub fn compute_revision_for_x(
    strip_left_dip: f32,
    strip_right_dip: f32,
    earliest: Revision,
    head: Revision,
    x_dip: f32,
) -> Revision {
    let left = earliest.get();
    let right = head.get();
    if right <= left || strip_right_dip <= strip_left_dip {
        return head;
    }
    let span_dip = strip_right_dip - strip_left_dip;
    let clamped = x_dip.clamp(strip_left_dip, strip_right_dip);
    let normalized = (clamped - strip_left_dip) / span_dip;
    let revision_span = right - left;
    let offset = (normalized * revision_span as f32).round() as u64;
    Revision(left.saturating_add(offset).min(right))
}

/// Map a revision to its `x_dip` coordinate inside the slider strip.
/// Revisions outside `[earliest, head]` are clamped to the strip's
/// edges.
#[must_use]
pub fn compute_x_for_revision(
    strip_left_dip: f32,
    strip_right_dip: f32,
    earliest: Revision,
    head: Revision,
    revision: Revision,
) -> f32 {
    let left = earliest.get();
    let right = head.get();
    if right <= left || strip_right_dip <= strip_left_dip {
        return strip_left_dip;
    }
    let revision_span = (right - left) as f32;
    let target = revision.get().clamp(left, right);
    let normalized = (target - left) as f32 / revision_span;
    strip_left_dip + normalized * (strip_right_dip - strip_left_dip)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summaries() -> Vec<SnapshotSummaryRow> {
        vec![
            SnapshotSummaryRow {
                revision: Revision(0),
                created_at_ms: 1_000,
                label: None,
            },
            SnapshotSummaryRow {
                revision: Revision(50),
                created_at_ms: 2_000,
                label: Some("pre-refactor".into()),
            },
            SnapshotSummaryRow {
                revision: Revision(100),
                created_at_ms: 3_000,
                label: None,
            },
        ]
    }

    #[test]
    fn revision_for_x_clamps_outside_strip_to_edges() {
        let earliest = Revision(0);
        let head = Revision(100);
        assert_eq!(
            compute_revision_for_x(100.0, 200.0, earliest, head, 50.0),
            earliest
        );
        assert_eq!(
            compute_revision_for_x(100.0, 200.0, earliest, head, 250.0),
            head
        );
    }

    #[test]
    fn revision_for_x_midpoint_is_midpoint_revision() {
        let r = compute_revision_for_x(100.0, 200.0, Revision(0), Revision(100), 150.0);
        assert_eq!(r, Revision(50));
    }

    #[test]
    fn revision_for_x_collapses_when_head_equals_earliest() {
        let r = compute_revision_for_x(0.0, 100.0, Revision(7), Revision(7), 50.0);
        assert_eq!(r, Revision(7));
    }

    #[test]
    fn x_for_revision_inverts_revision_for_x_at_endpoints() {
        let earliest = Revision(0);
        let head = Revision(100);
        assert!(
            (compute_x_for_revision(100.0, 200.0, earliest, head, earliest) - 100.0).abs() < 0.001
        );
        assert!((compute_x_for_revision(100.0, 200.0, earliest, head, head) - 200.0).abs() < 0.001);
    }

    #[test]
    fn build_geometry_lays_out_ticks_in_ascending_x_order() {
        let g = SliderGeometry::build(
            400.0,
            300.0,
            Revision(0),
            Revision(100),
            Revision(100),
            &summaries(),
        );
        assert_eq!(g.ticks.len(), 3);
        assert!(g.ticks[0].x_dip < g.ticks[1].x_dip);
        assert!(g.ticks[1].x_dip < g.ticks[2].x_dip);
    }

    #[test]
    fn build_geometry_in_rect_places_band_at_pane_body_bottom() {
        let g = SliderGeometry::build_in_rect(
            SliderPaneRect {
                left_dip: 400.0,
                top_dip: 300.0,
                width_dip: 320.0,
                height_dip: 240.0,
            },
            Revision(0),
            Revision(100),
            Revision(100),
            &summaries(),
        );
        assert_eq!(g.band_bottom_dip, 540.0);
        assert!(g.band_top_dip >= 300.0);
        assert!(g.strip_left_dip >= 400.0);
        assert!(g.strip_right_dip <= 720.0);
    }

    #[test]
    fn build_geometry_marks_named_vs_edit_only_ticks() {
        let g = SliderGeometry::build(
            400.0,
            300.0,
            Revision(0),
            Revision(100),
            Revision(100),
            &summaries(),
        );
        assert_eq!(g.ticks[0].kind, TickKind::EditOnlySnapshot);
        assert_eq!(g.ticks[1].kind, TickKind::NamedSnapshot);
        assert_eq!(g.ticks[1].label.as_deref(), Some("pre-refactor"));
        assert_eq!(g.ticks[2].kind, TickKind::EditOnlySnapshot);
    }

    #[test]
    fn hit_test_outside_band_is_outside() {
        let g = SliderGeometry::build(
            400.0,
            300.0,
            Revision(0),
            Revision(100),
            Revision(100),
            &summaries(),
        );
        assert_eq!(g.hit_test(200.0, 0.0), SliderHit::Outside);
    }

    #[test]
    fn hit_test_on_thumb_returns_thumb() {
        let g = SliderGeometry::build(
            400.0,
            300.0,
            Revision(0),
            Revision(100),
            Revision(50),
            &summaries(),
        );
        let thumb_x = g.thumb_x_dip();
        let center_y = g.strip_center_y_dip;
        assert_eq!(g.hit_test(thumb_x, center_y), SliderHit::Thumb);
    }

    #[test]
    fn hit_test_on_named_tick_returns_tick() {
        let g = SliderGeometry::build(
            400.0,
            300.0,
            Revision(0),
            Revision(100),
            // Position the thumb at head so the named tick at rev 50
            // is not occluded by the thumb hit-test (thumb wins ties).
            Revision(100),
            &summaries(),
        );
        let named_tick = g
            .ticks
            .iter()
            .find(|t| t.kind == TickKind::NamedSnapshot)
            .unwrap()
            .clone();
        let center_y = g.strip_center_y_dip;
        match g.hit_test(named_tick.x_dip, center_y) {
            SliderHit::Tick(t) => {
                assert_eq!(t.revision, Revision(50));
                assert_eq!(t.label.as_deref(), Some("pre-refactor"));
            }
            other => panic!("expected Tick, got {other:?}"),
        }
    }

    #[test]
    fn hit_test_in_track_gutter_returns_track_revision() {
        let g = SliderGeometry::build(
            400.0,
            300.0,
            Revision(0),
            Revision(100),
            // Park thumb at the right end so the gutter near the left
            // is empty.
            Revision(100),
            &[],
        );
        let center_y = g.strip_center_y_dip;
        match g.hit_test(g.strip_left_dip + 5.0, center_y) {
            SliderHit::Track { revision } => {
                // Near-left of the strip should map to a low revision.
                assert!(revision <= Revision(20));
            }
            other => panic!("expected Track, got {other:?}"),
        }
    }

    #[test]
    fn has_drag_range_false_when_buffer_has_one_revision() {
        let g = SliderGeometry::build(400.0, 300.0, Revision(7), Revision(7), Revision(7), &[]);
        assert!(!g.has_drag_range());
    }
}
