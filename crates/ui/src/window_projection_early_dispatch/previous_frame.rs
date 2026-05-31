use crate::display_prewarm_cache::PrewarmQuery;
use continuity_render::FrameDisplay;

pub(super) fn compatible_last_painted_frame<'a>(
    candidate: Option<&'a (PrewarmQuery, FrameDisplay)>,
    display_query: &PrewarmQuery,
) -> Option<(&'a PrewarmQuery, &'a FrameDisplay)> {
    candidate.and_then(|(query, frame)| {
        query
            .hit_test_compat_mismatch(display_query)
            .is_none()
            .then_some((query, frame))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use continuity_buffer::BufferId;
    use continuity_display_map::wrap::FixedCharWidth;
    use continuity_layout::FontStateId;
    use ropey::Rope;

    fn query(buffer_id: BufferId, revision: u64, wrap_width_dip: u32) -> PrewarmQuery {
        PrewarmQuery::new(
            buffer_id,
            revision,
            None,
            &[0],
            &[],
            wrap_width_dip,
            FontStateId::default(),
        )
    }

    fn tiny_frame_display(revision: u64) -> FrameDisplay {
        let rope = Rope::from_str("a\nb\nc\n");
        let mut measure = FixedCharWidth::new(8.0);
        FrameDisplay::build_viewport_measured(
            &rope,
            revision,
            None,
            &[0usize],
            &[],
            &[],
            0,
            &mut measure,
            0..3,
            0,
        )
    }

    #[test]
    fn rejects_frame_from_another_document_before_classification() {
        let candidate = (query(BufferId::new(), 1, 480), tiny_frame_display(1));
        let current = query(BufferId::new(), 1, 480);
        assert!(compatible_last_painted_frame(Some(&candidate), &current).is_none());
    }

    #[test]
    fn accepts_same_document_with_different_revision() {
        let buffer_id = BufferId::new();
        let candidate = (query(buffer_id, 1, 480), tiny_frame_display(1));
        let current = query(buffer_id, 7, 480);
        assert!(compatible_last_painted_frame(Some(&candidate), &current).is_some());
    }

    #[test]
    fn rejects_same_document_with_different_wrap_geometry() {
        let buffer_id = BufferId::new();
        let candidate = (query(buffer_id, 1, 480), tiny_frame_display(1));
        let current = query(buffer_id, 1, 360);
        assert!(compatible_last_painted_frame(Some(&candidate), &current).is_none());
    }
}
