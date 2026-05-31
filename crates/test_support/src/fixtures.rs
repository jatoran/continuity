//! Canned buffers for tests.

use continuity_buffer::Buffer;

/// A simple `Buffer` containing `"Hello, world!"`.
#[must_use]
pub fn hello_world() -> Buffer {
    Buffer::from_text("Hello, world!")
}

/// A `Buffer` populated with a small markdown sample exercising headings,
/// lists, code blocks, and emphasis.
#[must_use]
pub fn golden_markdown() -> Buffer {
    const SRC: &str = "\
# Heading

A paragraph with **bold** and *italic*.

- item one
- item two

```rust
fn main() {
    println!(\"hi\");
}
```

> quote
";
    Buffer::from_text(SRC)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_world_has_expected_content() {
        assert_eq!(hello_world().rope().to_string(), "Hello, world!");
    }

    #[test]
    fn golden_markdown_is_nonempty() {
        assert!(golden_markdown().rope().len_bytes() > 50);
    }
}
