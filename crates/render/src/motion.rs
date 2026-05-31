//! Paint-time motion metadata shared by chrome, overlays, status chips,
//! and jump acknowledgements.
//!
//! These are plain data structs. The UI layer owns scheduling and reduced-
//! motion policy; the renderer only applies the per-frame projection it is
//! handed.

/// Opacity/translation applied to a transient surface.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SurfaceMotion {
    /// Alpha multiplier in `[0, 1]`.
    pub opacity: f32,
    /// Vertical translation in DIPs.
    pub translate_y_dip: f32,
}

impl Default for SurfaceMotion {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl SurfaceMotion {
    /// No visual transform.
    pub const IDENTITY: Self = Self {
        opacity: 1.0,
        translate_y_dip: 0.0,
    };

    /// Construct a clamped motion projection.
    #[must_use]
    pub fn new(opacity: f32, translate_y_dip: f32) -> Self {
        Self {
            opacity: opacity.clamp(0.0, 1.0),
            translate_y_dip,
        }
    }

    /// `true` when painting can use the normal static path.
    #[must_use]
    pub fn is_identity(self) -> bool {
        (self.opacity - 1.0).abs() <= f32::EPSILON && self.translate_y_dip.abs() <= f32::EPSILON
    }
}

/// Status-bar side of a transient entry.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum StatusTransientGroup {
    /// Left-aligned status segment.
    Segment,
    /// Right-aligned warning chip.
    Chip,
}

/// Per-frame transient for one status-bar segment/chip.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct StatusTransientDraw {
    /// Segment group.
    pub group: StatusTransientGroup,
    /// Index within that group.
    pub index: usize,
    /// Alpha multiplier in `[0, 1]`.
    pub alpha: f32,
    /// Vertical translation in DIPs.
    pub translate_y_dip: f32,
}

/// Destination-row acknowledgement glow.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct JumpGlowDraw {
    /// Display-line index to tint.
    pub display_line: u32,
    /// Alpha multiplier in `[0, 1]`.
    pub alpha: f32,
    /// Glow color.
    pub color: crate::Rgba,
}

/// α.1 edit-action echo: a contiguous source-line range tinted briefly
/// after a structural edit (paste, duplicate, move-line, undo/redo) or a
/// smart-expand step. The UI layer owns the line range, fade alpha, and
/// kind classification; the renderer paints a flat tint across the
/// covered rows.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct EditPulseDraw {
    /// First source line of the affected range (inclusive).
    pub first_line: u32,
    /// Last source line of the affected range (inclusive). Equal to
    /// `first_line` for a single-line pulse.
    pub last_line: u32,
    /// Alpha multiplier in `[0, 1]`.
    pub alpha: f32,
    /// Tint color.
    pub color: crate::Rgba,
}
