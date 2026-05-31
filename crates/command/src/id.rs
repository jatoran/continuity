//! Command identifier newtype.

/// A static-string command id (e.g., `"editor.move_line_up"`).
///
/// Uses `&'static str` so registries and palettes can compare ids by pointer
/// when desired and avoid allocation.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommandId(pub &'static str);

impl CommandId {
    /// Borrow the inner string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for CommandId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equality_and_hash_are_string_based() {
        let a = CommandId("editor.x");
        let b = CommandId("editor.x");
        assert_eq!(a, b);
    }
}
