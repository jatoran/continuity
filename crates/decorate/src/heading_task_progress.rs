//! γ — task-list progress per markdown section.
//!
//! For each heading, walk the GFM task-list checkboxes that fall
//! within that heading's section bounds and tally `(done, total)`.
//! Pure data; consumers (outline-sidebar, sticky breadcrumb, body
//! paint overlay) format the ratio however suits their surface.
//!
//! Single-writer: pure functions, callable from any decoration
//! consumer that already holds the parsed [`crate::headings::HeadingEntry`]
//! list and the inline-span vector from [`crate::Decorations`].

use crate::headings::HeadingEntry;
use crate::inline::{InlineKind, InlineSpan};
use crate::sections::section_bounds;

/// Source extent for task-progress section bounds.
///
/// Accepts either an existing byte length or a rope reference so paint
/// callers do not need to clone the full source to compute EOF.
pub trait TaskProgressSourceExtent {
    /// Total source length in UTF-8 bytes.
    fn len_bytes(self) -> usize;
}

impl TaskProgressSourceExtent for usize {
    fn len_bytes(self) -> usize {
        self
    }
}

impl TaskProgressSourceExtent for &str {
    fn len_bytes(self) -> usize {
        self.len()
    }
}

impl TaskProgressSourceExtent for &ropey::Rope {
    fn len_bytes(self) -> usize {
        self.len_bytes()
    }
}

/// `(done, total)` task-list count for one heading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskProgress {
    /// Count of `[x]` checkboxes in this heading's section.
    pub done: u32,
    /// Count of `[ ]` + `[x]` checkboxes in this heading's section.
    pub total: u32,
}

impl TaskProgress {
    /// `true` when the section contains zero task-list checkboxes —
    /// the suffix should be suppressed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// `"<done>/<total>"` suffix string. Returns `None` when
    /// [`Self::is_empty`], so callers can `.map(format)` without an
    /// extra branch.
    #[must_use]
    pub fn format_suffix(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        Some(format!("{}/{}", self.done, self.total))
    }
}

/// One per heading, in the same order as the input `headings` slice.
/// Headings whose section contains no checkboxes receive `(0, 0)`.
#[must_use]
pub fn task_progress_per_heading(
    headings: &[HeadingEntry],
    inlines: &[InlineSpan],
    source_extent: impl TaskProgressSourceExtent,
) -> Vec<TaskProgress> {
    let source_len = source_extent.len_bytes();
    let mut out = Vec::with_capacity(headings.len());
    for idx in 0..headings.len() {
        let Some(bounds) = section_bounds(headings, idx, source_len) else {
            out.push(TaskProgress { done: 0, total: 0 });
            continue;
        };
        let mut done = 0u32;
        let mut total = 0u32;
        for span in inlines {
            if let InlineKind::Checkbox { checked, .. } = &span.kind {
                // Use the inline span's `range.start` as the locator;
                // the toggle_byte sits inside that range and is what
                // the user clicks, but the start byte is enough for a
                // section-containment check.
                let b = span.range.start;
                if b >= bounds.start_byte && b < bounds.end_byte {
                    total += 1;
                    if *checked {
                        done += 1;
                    }
                }
            }
        }
        out.push(TaskProgress { done, total });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inline::ByteRange;
    use ropey::Rope;

    fn heading(level: u8, text: &str, start_byte: usize) -> HeadingEntry {
        HeadingEntry {
            level,
            text: text.into(),
            start_byte,
            line: 0,
        }
    }

    fn checkbox(byte: usize, checked: bool) -> InlineSpan {
        InlineSpan {
            range: ByteRange {
                start: byte,
                end: byte + 3,
            },
            kind: InlineKind::Checkbox {
                checked,
                toggle_byte: byte + 1,
            },
        }
    }

    #[test]
    fn format_suffix_skips_empty() {
        let p = TaskProgress { done: 0, total: 0 };
        assert!(p.format_suffix().is_none());
        let p = TaskProgress { done: 2, total: 5 };
        assert_eq!(p.format_suffix().as_deref(), Some("2/5"));
    }

    #[test]
    fn one_section_tallies_done_and_total() {
        let headings = vec![heading(1, "Tasks", 0)];
        let inlines = vec![checkbox(10, false), checkbox(20, true), checkbox(30, true)];
        let progress = task_progress_per_heading(&headings, &inlines, 100);
        assert_eq!(progress.len(), 1);
        assert_eq!(progress[0].done, 2);
        assert_eq!(progress[0].total, 3);
    }

    #[test]
    fn nested_section_counts_belong_to_subheading() {
        // h1 at 0; sibling h2 "Tasks" at 50 closes at the next h2 at 200.
        let headings = vec![
            heading(1, "Project", 0),
            heading(2, "Tasks", 50),
            heading(2, "Notes", 200),
        ];
        let inlines = vec![
            checkbox(60, false),  // inside Tasks
            checkbox(70, true),   // inside Tasks
            checkbox(210, false), // inside Notes
        ];
        let progress = task_progress_per_heading(&headings, &inlines, 300);
        // Project (h1) runs to EOF, swallowing everything.
        assert_eq!(progress[0].done, 1);
        assert_eq!(progress[0].total, 3);
        // Tasks covers [50, 200).
        assert_eq!(progress[1].done, 1);
        assert_eq!(progress[1].total, 2);
        // Notes covers [200, 300).
        assert_eq!(progress[2].done, 0);
        assert_eq!(progress[2].total, 1);
    }

    #[test]
    fn empty_headings_returns_empty_vec() {
        let progress = task_progress_per_heading(&[], &[], 0);
        assert!(progress.is_empty());
    }

    #[test]
    fn rope_extent_matches_source_length_extent() {
        let src = "# Project\n\n- [ ] todo\n- [x] done\n";
        let rope = Rope::from_str(src);
        let headings = vec![heading(1, "Project", 0)];
        let inlines = vec![checkbox(11, false), checkbox(22, true)];

        assert_eq!(
            task_progress_per_heading(&headings, &inlines, src.len()),
            task_progress_per_heading(&headings, &inlines, &rope),
        );
    }
}
