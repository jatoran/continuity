//! Platform-neutral accessibility state for one editor surface.

use std::sync::{Arc, Mutex};

use continuity_buffer::RopeSnapshot;
use continuity_core::EditorSnapshot;
use continuity_engine::EngineSnapshot;
use continuity_text::Selection;

/// Immutable document data exposed to a platform accessibility adapter.
#[derive(Clone)]
pub(crate) struct AccessibilityDocument {
    /// Canonical rope and revision.
    pub(crate) rope: RopeSnapshot,
    /// Canonical selection set captured with the rope.
    pub(crate) selections: Vec<Selection>,
    /// Whether text mutation is disabled for this document.
    pub(crate) is_read_only: bool,
}

/// Cross-thread snapshot read by native accessibility providers.
#[derive(Clone, Default)]
pub(crate) struct AccessibilitySnapshot {
    /// Current document, absent before the first surface publication.
    pub(crate) document: Option<AccessibilityDocument>,
    /// Whether the editor control accepts interaction.
    pub(crate) is_enabled: bool,
    /// Whether the editor control owns keyboard focus.
    pub(crate) has_keyboard_focus: bool,
}

/// Shared accessibility state owned by one editor surface.
///
/// The UI thread is the sole writer. UI Automation may query providers from
/// another COM thread, so the adapter reads short-lived immutable clones under
/// this mutex; it never holds the lock while converting or returning text.
pub(crate) struct AccessibilityState {
    shared: Arc<Mutex<AccessibilitySnapshot>>,
}

impl AccessibilityState {
    /// Construct enabled state before a document is first published.
    pub(crate) fn new() -> Self {
        Self {
            shared: Arc::new(Mutex::new(AccessibilitySnapshot {
                document: None,
                is_enabled: true,
                has_keyboard_focus: true,
            })),
        }
    }

    /// Clone the synchronized state handle for a platform adapter.
    pub(crate) fn shared(&self) -> Arc<Mutex<AccessibilitySnapshot>> {
        Arc::clone(&self.shared)
    }

    /// Publish the latest canonical editor snapshot and surface semantics.
    ///
    /// Returns the previous revision and selection set so the native adapter
    /// can raise precise text/selection notifications without duplicating
    /// document state.
    pub(crate) fn publish(
        &self,
        snapshot: &EditorSnapshot,
        is_enabled: bool,
        has_keyboard_focus: bool,
    ) -> AccessibilityChange {
        self.publish_parts(
            snapshot.rope.clone(),
            snapshot.selections.clone(),
            snapshot.is_read_only,
            is_enabled,
            has_keyboard_focus,
        )
    }

    /// Publish a storage-neutral engine snapshot for an embedded surface.
    pub(crate) fn publish_engine(
        &self,
        snapshot: &EngineSnapshot,
        is_enabled: bool,
        has_keyboard_focus: bool,
    ) -> AccessibilityChange {
        self.publish_parts(
            snapshot.rope.clone(),
            snapshot.selections.clone(),
            snapshot.is_read_only,
            is_enabled,
            has_keyboard_focus,
        )
    }

    fn publish_parts(
        &self,
        rope: RopeSnapshot,
        selections: Vec<Selection>,
        is_read_only: bool,
        is_enabled: bool,
        has_keyboard_focus: bool,
    ) -> AccessibilityChange {
        let mut shared = self
            .shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = shared.document.as_ref();
        let change = AccessibilityChange {
            was_text_changed: previous
                .is_some_and(|document| document.rope.revision() != rope.revision()),
            were_selections_changed: previous
                .is_some_and(|document| document.selections != selections),
            was_focus_changed: shared.has_keyboard_focus != has_keyboard_focus,
        };
        shared.document = Some(AccessibilityDocument {
            rope,
            selections,
            is_read_only,
        });
        shared.is_enabled = is_enabled;
        shared.has_keyboard_focus = has_keyboard_focus;
        change
    }

    /// Current published revision, used by focused tests and diagnostics.
    #[cfg(test)]
    pub(crate) fn revision(&self) -> Option<continuity_buffer::Revision> {
        self.shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .document
            .as_ref()
            .map(|document| document.rope.revision())
    }
}

/// Semantic changes observed during one accessibility publication.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct AccessibilityChange {
    /// The canonical rope revision changed.
    pub(crate) was_text_changed: bool,
    /// The canonical selection set changed.
    pub(crate) were_selections_changed: bool,
    /// Keyboard-focus state changed.
    pub(crate) was_focus_changed: bool,
}
