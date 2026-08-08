//! Final materialized-row count for the allocation-light wrap walker.

pub(super) fn compute_materialized_rows(
    breaks: u16,
    last_cut: Option<usize>,
    display_byte_len: usize,
) -> u16 {
    // `soft_wrap_spec` drops a terminal break because splitting at the
    // display-string end would create an empty row. Mirror that rule.
    let terminal_break = u16::from(last_cut == Some(display_byte_len));
    breaks.saturating_add(1).saturating_sub(terminal_break)
}
