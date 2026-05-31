//! Spectator-pane body margin and text-width geometry.

use crate::chrome::{ContentMargins, BODY_LEFT_PADDING_DIP, GUTTER_BODY_GAP_DIP};

/// Width of the text column inside a non-focused pane body without
/// right-edge chrome.
#[must_use]
pub(super) fn spectator_body_text_width_dip(
    width: f32,
    font_size_dip: f32,
    line_numbers: bool,
) -> f32 {
    spectator_body_text_width_for_line_count_dip(width, font_size_dip, line_numbers, 99)
}

/// Width of the text column inside a non-focused pane body for a
/// specific buffer line count.
#[must_use]
pub(super) fn spectator_body_text_width_for_line_count_dip(
    width: f32,
    font_size_dip: f32,
    line_numbers: bool,
    source_line_count: usize,
) -> f32 {
    let margins = spectator_content_margins(line_numbers, width, font_size_dip, source_line_count);
    (width.max(1.0) - margins.left - margins.right).max(1.0)
}

/// Width of the text column inside a non-focused pane body when
/// right-edge chrome is globally visible.
#[must_use]
pub(super) fn spectator_body_text_width_with_right_edge_dip(
    width: f32,
    font_size_dip: f32,
    line_numbers: bool,
    minimap: bool,
    show_outline_sidebar: bool,
    outline_sidebar_width_dip: f32,
) -> f32 {
    spectator_body_text_width_with_right_edge_for_line_count_dip(
        width,
        font_size_dip,
        line_numbers,
        99,
        minimap,
        show_outline_sidebar,
        outline_sidebar_width_dip,
    )
}

/// Width of the text column inside a non-focused pane body when
/// right-edge chrome is globally visible, using a specific buffer line
/// count for the gutter.
#[must_use]
pub(super) fn spectator_body_text_width_with_right_edge_for_line_count_dip(
    width: f32,
    font_size_dip: f32,
    line_numbers: bool,
    source_line_count: usize,
    minimap: bool,
    show_outline_sidebar: bool,
    outline_sidebar_width_dip: f32,
) -> f32 {
    let margins = spectator_content_margins_with_right_edge(
        line_numbers,
        width,
        font_size_dip,
        source_line_count,
        minimap,
        show_outline_sidebar,
        outline_sidebar_width_dip,
    );
    (width.max(1.0) - margins.left - margins.right).max(1.0)
}

fn spectator_content_margins(
    line_numbers: bool,
    width: f32,
    font_size_dip: f32,
    source_line_count: usize,
) -> ContentMargins {
    spectator_content_margins_with_right_edge(
        line_numbers,
        width,
        font_size_dip,
        source_line_count,
        false,
        false,
        0.0,
    )
}

pub(super) fn spectator_content_margins_with_right_edge(
    line_numbers: bool,
    width: f32,
    font_size_dip: f32,
    source_line_count: usize,
    minimap: bool,
    show_outline_sidebar: bool,
    outline_sidebar_width_dip: f32,
) -> ContentMargins {
    let left = if line_numbers {
        crate::chrome::gutter_width_for_line_count(font_size_dip, source_line_count)
            + GUTTER_BODY_GAP_DIP
    } else {
        BODY_LEFT_PADDING_DIP
    };
    let right = crate::chrome::resolve_body_right_margin_dip(
        minimap,
        false,
        show_outline_sidebar,
        outline_sidebar_width_dip,
    )
    .min((width - left).max(0.0));
    ContentMargins { left, right }
}
