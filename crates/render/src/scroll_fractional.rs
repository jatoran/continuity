//! Fractional scroll helpers.
//!
//! The renderer already carries scroll positions as DIPs; these helpers
//! make the sub-pixel part explicit and keep tests close to the paint
//! math that translates body rows by `-scroll_y_dip`.

/// Fractional portion of a vertical scroll offset in DIPs.
#[must_use]
pub fn fractional_scroll_y_dip(scroll_y_dip: f32) -> f32 {
    scroll_y_dip - scroll_y_dip.floor()
}

/// Body translation delta caused only by the fractional part of scroll.
#[must_use]
pub fn fractional_body_translation_y_dip(scroll_y_dip: f32) -> f32 {
    -fractional_scroll_y_dip(scroll_y_dip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_dip_scroll_translates_body_by_half_dip() {
        let whole = fractional_body_translation_y_dip(100.0);
        let half = fractional_body_translation_y_dip(100.5);
        assert!((half - whole + 0.5).abs() < f32::EPSILON);
    }
}
