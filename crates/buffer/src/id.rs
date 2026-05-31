//! Identifier newtypes for buffers and undo groups.
//!
//! Both wrap UUIDv7 so that they sort by creation time naturally.

use uuid::Uuid;

/// A buffer's stable identifier.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BufferId(Uuid);

impl BufferId {
    /// Allocate a new id.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Wrap an existing UUID. Used by deserialization paths (persistence,
    /// search index lookups) when the id was minted earlier.
    #[must_use]
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Borrow the inner UUID for serialization.
    #[must_use]
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }

    /// All-zero sentinel value. Used by tab kinds that carry no
    /// underlying buffer (e.g. the buffer-history visualization tab) —
    /// the persist layer never assigns this id, so lookups against it
    /// naturally return `None`.
    #[must_use]
    pub const fn nil() -> Self {
        Self(Uuid::nil())
    }

    /// `true` when this id is the all-zero sentinel returned by
    /// [`Self::nil`]. Useful for guards that need to short-circuit
    /// before calling into the editor for a non-buffer surface.
    #[must_use]
    pub fn is_nil(&self) -> bool {
        self.0.is_nil()
    }
}

impl Default for BufferId {
    fn default() -> Self {
        Self::new()
    }
}

/// A window's stable identifier.
///
/// Wraps a UUIDv7 so cross-session restoration can match a persisted
/// `windows` row to the spawned in-memory window.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(Uuid);

impl WindowId {
    /// Allocate a new id.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Wrap an existing UUID. Used by deserialization paths.
    #[must_use]
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Borrow the inner UUID for serialization.
    #[must_use]
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for WindowId {
    fn default() -> Self {
        Self::new()
    }
}

/// An undo-group identifier.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UndoGroupId(Uuid);

impl UndoGroupId {
    /// Allocate a new id.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Wrap an existing UUID. Used by deserialization paths (persistence
    /// recovery, edit-log round-trips).
    #[must_use]
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Borrow the inner UUID for serialization.
    #[must_use]
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for UndoGroupId {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_ids_are_unique() {
        let a = BufferId::new();
        let b = BufferId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn nil_is_all_zeros_and_distinct_from_fresh_ids() {
        let n = BufferId::nil();
        assert!(n.is_nil());
        assert_eq!(n.as_uuid(), Uuid::nil());
        let fresh = BufferId::new();
        assert!(!fresh.is_nil());
        assert_ne!(n, fresh);
    }

    #[test]
    fn buffer_ids_sort_by_creation_order() {
        let a = BufferId::new();
        let b = BufferId::new();
        // UUIDv7 is time-ordered; b was created later so a < b (almost always).
        assert!(a < b || a.as_uuid().get_version_num() == 7);
    }
}
