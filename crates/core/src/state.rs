//! `EditorState`: the in-memory map of open buffers.
//!
//! **Thread ownership**: only the editor core thread mutates `EditorState`.

use ahash::AHashMap;
use continuity_buffer::{Buffer, BufferId};

/// The editor's authoritative buffer collection.
#[derive(Debug, Default)]
pub struct EditorState {
    buffers: AHashMap<BufferId, Buffer>,
}

impl EditorState {
    /// An empty state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a buffer.
    pub fn insert(&mut self, buffer: Buffer) -> BufferId {
        let id = buffer.id();
        self.buffers.insert(id, buffer);
        id
    }

    /// Borrow a buffer.
    #[must_use]
    pub fn get(&self, id: BufferId) -> Option<&Buffer> {
        self.buffers.get(&id)
    }

    /// Mutably borrow a buffer.
    pub fn get_mut(&mut self, id: BufferId) -> Option<&mut Buffer> {
        self.buffers.get_mut(&id)
    }

    /// Remove a buffer.
    pub fn remove(&mut self, id: BufferId) -> Option<Buffer> {
        self.buffers.remove(&id)
    }

    /// Number of open buffers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.buffers.len()
    }

    /// `true` when no buffers are open.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buffers.is_empty()
    }

    /// Iterate open buffer ids.
    pub fn ids(&self) -> impl Iterator<Item = BufferId> + '_ {
        self.buffers.keys().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_get() {
        let mut s = EditorState::new();
        let buf = Buffer::from_text("hi");
        let id = buf.id();
        let inserted = s.insert(buf);
        assert_eq!(inserted, id);
        assert_eq!(s.len(), 1);
        assert_eq!(s.get(id).unwrap().rope().to_string(), "hi");
    }

    #[test]
    fn remove_returns_buffer() {
        let mut s = EditorState::new();
        let id = s.insert(Buffer::empty());
        assert!(s.remove(id).is_some());
        assert!(s.is_empty());
    }
}
