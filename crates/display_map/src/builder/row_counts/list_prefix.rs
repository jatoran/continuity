//! Display-space list-prefix discovery for the allocation-light row walker.

use crate::id::SourceByte;
use crate::segment::DisplaySegment;
use crate::wrap::list_item_content_start_byte;

pub(super) fn compute_list_prefix_end(
    segments: &[DisplaySegment],
    line_text: &str,
    source_byte_start: SourceByte,
) -> Option<usize> {
    let display_text = segments
        .iter()
        .map(|segment| segment.display_bytes(line_text, source_byte_start))
        .collect::<String>();
    list_item_content_start_byte(&display_text)
}
