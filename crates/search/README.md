# search

Literal and regex matching for find, plus fuzzy scoring for
palette-style pickers. Literal-mode queries route through the
`memchr::memmem` dispatcher fast path; regex-mode queries and
non-ASCII case-insensitive literals stay on `grep-regex`.

Cross-buffer FTS5 content indexing is removed; `index.rs` is a legacy
no-op title-search stub. The search crate is stateless for live find
queries and can be called from UI or worker code.
