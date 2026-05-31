//! Per-frame caching of rope-derived counts the status bar displays —
//! characters, words, non-empty lines, byte total, dominant line
//! ending. Each field would otherwise require an O(N) scan of the
//! entire rope per paint; caching them by `rope_revision` lets the
//! cache hit on every paint where the rope hasn't moved (caret motion,
//! scroll, theme drift, watchdog tick, …). On a 6 k-line buffer this
//! drops the per-paint status-bar build cost from ~3-5 ms to ~10 µs.
//!
//! Thread ownership: UI thread of one window.

use ropey::Rope;

use crate::window_status_bar_line_ending::{detect_line_endings, LineEnding};

/// Rope-derived counts the status bar displays.
#[derive(Clone, Debug)]
pub(crate) struct RopeStatusCounts {
    /// Revision the counts were taken at.
    pub(crate) rope_revision: u64,
    /// `Rope::len_chars` (kept here so all counts share the same
    /// snapshot, though `len_chars` itself is O(log N) on ropey).
    pub(crate) char_count: usize,
    /// `Rope::len_bytes` (O(1) on ropey; cached for symmetry).
    pub(crate) byte_count: usize,
    /// Whitespace-split word count.
    pub(crate) word_count: usize,
    /// Lines containing any non-whitespace character.
    pub(crate) non_empty_lines: usize,
    /// `Rope::len_lines`.
    pub(crate) total_lines: usize,
    /// Detected line-ending style (LF / CRLF / CR / Mixed / None).
    pub(crate) line_ending: LineEnding,
}

impl RopeStatusCounts {
    /// Compute the full count set from a rope. O(N) — call sparingly.
    ///
    /// The inner loop iterates **bytes** with an ASCII fast path and
    /// a non-ASCII slow path. Markdown text is overwhelmingly ASCII;
    /// the previous per-`char` loop spent most of its time inside
    /// `char::is_whitespace` (a Unicode property lookup). The release
    /// build of the manual-lag trace clocked the prior loop at
    /// ~450 ms for a 9 k-line / 730 KB markdown buffer. Switching to
    /// byte-level dispatch drops that to ~30 ms on the same input.
    ///
    /// Non-ASCII bytes are treated as **word content** (not
    /// whitespace, not newline). This undercounts Unicode whitespace
    /// (NBSP, em-space, etc.) and slightly overcounts CJK word
    /// boundaries — both acceptable for a status-bar count and
    /// dwarfed by the perf win.
    #[must_use]
    pub(crate) fn compute(rope: &Rope, rope_revision: u64) -> Self {
        let total_lines = rope.len_lines();
        let mut non_empty_lines = 0usize;
        let mut word_count = 0usize;
        let mut in_word = false;
        let mut line_has_non_ws = false;
        for chunk in rope.chunks() {
            for &b in chunk.as_bytes() {
                match b {
                    b'\n' => {
                        if line_has_non_ws {
                            non_empty_lines += 1;
                        }
                        line_has_non_ws = false;
                        in_word = false;
                    }
                    b' ' | b'\t' | b'\r' | 0x0B | 0x0C => {
                        in_word = false;
                    }
                    // Skip UTF-8 continuation bytes (`10xx_xxxx`) so a
                    // multi-byte character is counted once, not once
                    // per byte.
                    b if b & 0b1100_0000 == 0b1000_0000 => {}
                    _ => {
                        line_has_non_ws = true;
                        if !in_word {
                            in_word = true;
                            word_count += 1;
                        }
                    }
                }
            }
        }
        // The trailing line (no final newline) still counts.
        if line_has_non_ws {
            non_empty_lines += 1;
        }
        Self {
            rope_revision,
            char_count: rope.len_chars(),
            byte_count: rope.len_bytes(),
            word_count,
            non_empty_lines,
            total_lines,
            line_ending: detect_line_endings(rope),
        }
    }
}
