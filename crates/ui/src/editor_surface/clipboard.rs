//! Clipboard session state and platform-boundary text normalization.

use std::collections::VecDeque;

/// Default depth of the in-memory paste-history ring.
const PASTE_HISTORY_CAPACITY: usize = 16;

/// Clipboard state whose lifetime belongs to one editor surface.
///
/// **Thread ownership:** the surface's UI thread is the sole writer. Native
/// clipboard handles and format probing remain in the Win32 host adapter; no
/// clipboard content is persisted.
#[derive(Debug, Default)]
pub(crate) struct ClipboardState {
    history: VecDeque<String>,
}

impl ClipboardState {
    /// Create an empty clipboard session.
    pub(crate) fn new() -> Self {
        Self {
            history: VecDeque::with_capacity(PASTE_HISTORY_CAPACITY),
        }
    }

    /// Remember copied or cut text, newest first.
    pub(crate) fn remember(&mut self, text: String) {
        if text.is_empty() || self.history.front().map(String::as_str) == Some(text.as_str()) {
            return;
        }
        self.history.push_front(text);
        while self.history.len() > PASTE_HISTORY_CAPACITY {
            self.history.pop_back();
        }
    }

    /// Borrow a history entry, where zero is the newest item.
    pub(crate) fn history_entry(&self, index: usize) -> Option<&str> {
        self.history.get(index).map(String::as_str)
    }
}

/// Normalize platform line endings to the rope's canonical `\n`.
pub(crate) fn normalize_line_endings(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_string();
    }
    let mut normalized = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                normalized.push('\n');
            }
            other => normalized.push(other),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::{normalize_line_endings, ClipboardState, PASTE_HISTORY_CAPACITY};

    #[test]
    fn history_skips_empty_and_immediate_duplicates() {
        let mut state = ClipboardState::new();
        state.remember("foo".into());
        state.remember("foo".into());
        state.remember("bar".into());
        state.remember(String::new());

        assert_eq!(state.history_entry(0), Some("bar"));
        assert_eq!(state.history_entry(1), Some("foo"));
        assert!(state.history_entry(2).is_none());
    }

    #[test]
    fn history_drops_oldest_entry_at_capacity() {
        let mut state = ClipboardState::new();
        for index in 0..(PASTE_HISTORY_CAPACITY + 4) {
            state.remember(index.to_string());
        }

        assert_eq!(
            state.history_entry(0),
            Some((PASTE_HISTORY_CAPACITY + 3).to_string().as_str())
        );
        assert!(state.history_entry(PASTE_HISTORY_CAPACITY).is_none());
    }

    #[test]
    fn line_endings_normalize_at_platform_boundary() {
        assert_eq!(normalize_line_endings("a\r\nb\rc"), "a\nb\nc");
        assert_eq!(normalize_line_endings("a\nb"), "a\nb");
    }
}
