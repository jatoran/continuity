//! Reusable editor-surface state owned by one UI thread.
//!
//! This Milestone 5 boundary contains state whose lifetime belongs to an
//! editor surface rather than to the desktop shell: input/composition state,
//! focused-pane viewport and reveal coordination, caret presentation,
//! projection scheduling, and derived-frame caches. HWND/application lifecycle
//! and desktop session state stay on [`crate::Window`].

use continuity_input::KeyChord;
use continuity_layout::{DWriteFactory, FontStateId, ViewState};

pub(crate) mod accessibility;
pub(crate) mod clipboard;
pub(crate) mod focus;
pub(crate) mod pointer;
pub(crate) mod projection;
pub(crate) mod render;
pub(crate) mod selection;
pub(crate) mod selection_dispatch;

/// Editor-local input and composition state.
///
/// **Thread ownership:** the UI thread that owns the containing surface is
/// the sole writer. No member crosses threads.
pub(crate) struct EditorSurface {
    /// Canonical state published to platform accessibility adapters.
    pub(crate) accessibility: accessibility::AccessibilityState,
    /// Clipboard history and canonical text-boundary handling.
    pub(crate) clipboard: clipboard::ClipboardState,
    /// Editor-control and nested overlay-input focus.
    pub(crate) focus: focus::FocusState,
    /// Sticky selection motion and last-edit navigation memory.
    pub(crate) selection: selection::SelectionState,
    /// DirectWrite, renderer, layout-cache, and table-layout state.
    pub(crate) render: render::RenderState,
    /// Projection worker, prewarm queue, and derived-frame caches.
    pub(crate) projection: projection::ProjectionState,
    /// Focused-pane scroll, zoom, and soft-wrap state.
    pub(crate) view: ViewState,
    /// Whether the smooth-scroll timer is active.
    pub(crate) scroll_anim_active: bool,
    /// Wheel-inertia state driven by the smooth-scroll timer.
    pub(crate) scroll_inertia: crate::window_scroll::ScrollInertia,
    /// Whether the next paint must snap to the canonical document end.
    pub(crate) pending_doc_end_scroll: bool,
    /// Per-paint geometry-shift anchor and reveal handoff.
    pub(crate) geometry_anchor: crate::window_view::geometry_anchor::GeometryAnchorState,
    /// Bounded retry count for document-end snaps against partial indexes.
    pub(crate) pending_doc_end_scroll_attempts: u8,
    /// Remaining non-blocking polls for an off-thread large jump.
    pub(crate) jump_offthread_polls: u8,
    /// Chords typed since a multi-key sequence began.
    pub(crate) pending_chord_sequence: Vec<KeyChord>,
    /// Whether the physical Shift modifier is held.
    pub(crate) shift_held: bool,
    /// Whether the caret blink is in its visible phase.
    pub(crate) caret_blink_visible: bool,
    /// Tick of the most recent editor input.
    pub(crate) last_input_tick: u64,
    /// Whether the caret-blink timer is active.
    pub(crate) caret_blink_active: bool,
    /// Active native IME composition.
    pub(crate) ime: crate::window_ime::ImeState,
    /// One-shot acknowledgement glow for a long caret jump.
    pub(crate) jump_glow: Option<crate::jump_glow::JumpGlow>,
    /// One-shot source-line pulse after an edit or undo target change.
    pub(crate) edit_pulse: Option<crate::edit_pulse::EditPulse>,
    /// Active caret motion tween for a large edit-driven jump.
    pub(crate) caret_tween: Option<crate::caret_tween::CaretTween>,
    /// Editor-body pointer, hover, scrollbar, and selection-drag state.
    pub(crate) pointer: pointer::PointerState,
}

impl EditorSurface {
    /// Create idle state for a newly constructed editor surface.
    pub(crate) fn new(dwrite: DWriteFactory, font_state: FontStateId) -> Self {
        Self {
            accessibility: accessibility::AccessibilityState::new(),
            clipboard: clipboard::ClipboardState::new(),
            focus: focus::FocusState::default(),
            selection: selection::SelectionState::default(),
            render: render::RenderState::new(dwrite, font_state),
            projection: projection::ProjectionState::new(),
            view: ViewState::new(),
            scroll_anim_active: false,
            scroll_inertia: crate::window_scroll::ScrollInertia::default(),
            pending_doc_end_scroll: false,
            geometry_anchor: crate::window_view::geometry_anchor::GeometryAnchorState::default(),
            pending_doc_end_scroll_attempts: 0,
            jump_offthread_polls: 0,
            pending_chord_sequence: Vec::new(),
            shift_held: false,
            caret_blink_visible: true,
            last_input_tick: 0,
            caret_blink_active: false,
            ime: crate::window_ime::ImeState::default(),
            jump_glow: None,
            edit_pulse: None,
            caret_tween: None,
            pointer: pointer::PointerState::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::EditorSurface;
    use continuity_layout::{DWriteFactory, FontStateId};

    #[test]
    fn new_surface_starts_with_idle_input_state() {
        let font_state = FontStateId::from_parts("Cascadia Mono", 14.0, "en-us", 1.0);
        let dwrite = DWriteFactory::new().expect("DirectWrite factory should initialize");
        let surface = EditorSurface::new(dwrite, font_state);

        assert!(surface.render.renderer.is_none());
        assert_eq!(surface.accessibility.revision(), None);
        assert!(surface.render.text_format.is_none());
        assert_eq!(surface.render.font_state, font_state);
        assert!(surface.focus.has_keyboard_focus);
        assert!(!surface.focus.overlay_input_focused);
        assert!(surface.pending_chord_sequence.is_empty());
        assert!(surface.projection.projection_worker.is_none());
        assert_eq!(surface.projection.projection_request_seq, 0);
        assert_eq!(surface.view.scroll_y_dip, 0.0);
        assert!(!surface.scroll_anim_active);
        assert!(!surface.pending_doc_end_scroll);
        assert_eq!(surface.pending_doc_end_scroll_attempts, 0);
        assert_eq!(surface.jump_offthread_polls, 0);
        assert!(!surface.shift_held);
        assert!(surface.caret_blink_visible);
        assert_eq!(surface.last_input_tick, 0);
        assert!(!surface.caret_blink_active);
        assert!(surface.clipboard.history_entry(0).is_none());
        assert!(!surface.ime.composing);
        assert!(surface.selection.intended_columns.is_empty());
        assert!(surface.selection.intended_display_columns.is_empty());
        assert!(surface.selection.intended_columns_for.is_empty());
        assert!(surface.selection.last_edit_stack.is_empty());
        assert!(surface.jump_glow.is_none());
        assert!(surface.edit_pulse.is_none());
        assert!(surface.caret_tween.is_none());
        assert_eq!(surface.pointer.click_count, 0);
        assert!(surface.pointer.selection_drag_pane.is_none());
        assert!(surface.pointer.scrollbar_drag.is_none());
    }
}
