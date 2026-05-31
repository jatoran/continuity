//! `proptest` generators for `EditOp`s suitable for stress-testing the rope.
//!
//! The generators are *append-only* (insert at end) so the resulting sequence
//! can be replayed against any starting buffer without precomputing offsets.

use continuity_text::{EditOp, Position};
use proptest::prelude::*;

/// A small ASCII text suitable for inserting (5–20 chars, no newlines).
pub fn insert_text() -> impl Strategy<Value = String> {
    prop::string::string_regex("[a-zA-Z0-9 ]{5,20}").unwrap()
}

/// A simple append-style insert at position `at`.
pub fn append_op_at(line: u32, byte_in_line: u32) -> impl Strategy<Value = EditOp> {
    insert_text().prop_map(move |text| EditOp::insert(Position::new(line, byte_in_line), text))
}

#[cfg(test)]
mod tests {
    use continuity_buffer::Buffer;
    use proptest::test_runner::{Config, TestRunner};

    use super::*;

    #[test]
    fn generated_inserts_apply_cleanly() {
        let mut runner = TestRunner::new(Config::with_cases(20));
        runner
            .run(&insert_text(), |text| {
                let mut buf = Buffer::from_text("");
                let op = EditOp::insert(Position::ZERO, text.clone());
                buf.apply(&op).unwrap();
                prop_assert_eq!(buf.rope().to_string(), text);
                Ok(())
            })
            .unwrap();
    }
}
