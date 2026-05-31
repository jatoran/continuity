//! Inline-image cache accessors on [`crate::Renderer`].
//!
//! Pulled out of `renderer.rs` so the draw orchestrator stays under the
//! file-length cap. The renderer is UI-thread owned; the cache uses
//! interior mutability only to keep draw APIs on `&self`.

use crate::Renderer;

impl Renderer {
    /// F5 redesign: snapshot the most recent frame's collapsed
    /// affordance hit rects. Empty when the frame painted no collapsed
    /// images. The mouse handler iterates this slice in reverse
    /// (top-most painted wins) for click routing.
    pub fn image_hits(&self) -> std::cell::Ref<'_, Vec<crate::InlineImageHit>> {
        self.last_image_hits.borrow()
    }

    /// Snapshot of the most recent frame's inline-`` `code` `` hit
    /// rects. Empty when the visible body contained no inline-code
    /// spans (or none were paintable — e.g. every span had a caret
    /// inside, putting the run into raw-source reveal mode).
    pub fn inline_code_hits(&self) -> std::cell::Ref<'_, Vec<crate::InlineCodeHit>> {
        self.last_inline_code_hits.borrow()
    }

    /// Peek the native pixel dimensions of a previously decoded inline
    /// image. Returns `None` for paths that have not yet been decoded
    /// or have been evicted.
    #[must_use]
    pub fn cached_image_dimensions(&self, path: &std::path::Path) -> Option<(u32, u32)> {
        self.image_cache.borrow().cached_dimensions(path)
    }

    /// Update the inline-image bitmap cache cap.
    pub fn set_image_cache_capacity(&self, bytes: usize) {
        self.image_cache.borrow_mut().set_capacity_bytes(bytes);
    }

    /// Drop every cached image bitmap. Called after device-loss
    /// recovery; cached bitmaps belong to the old D2D device.
    pub fn invalidate_image_cache(&self) {
        self.image_cache.borrow_mut().invalidate_for_new_device();
    }

    /// Image-cache resident byte total. Surfaced via
    /// `event:memory_breakdown` for memory-attribution diagnostics.
    #[must_use]
    pub fn image_cache_current_bytes(&self) -> usize {
        self.image_cache.borrow().current_bytes()
    }

    /// Advance every animated entry; returns `true` if any frame
    /// changed.
    pub fn advance_image_animations(&self, now_ms: u64) -> bool {
        self.image_cache.borrow_mut().advance_animations(now_ms)
    }

    /// Returns `true` when the cache holds at least one animated image.
    #[must_use]
    pub fn has_animated_images(&self) -> bool {
        self.image_cache.borrow().has_animated_entries()
    }
}
