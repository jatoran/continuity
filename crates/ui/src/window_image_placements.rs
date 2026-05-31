//! Phase F5 Pass 2 — per-frame builder for `InlineImagePlacement`s.
//!
//! Iterates the focused buffer's decoration cache, picks out every
//! `InlineKind::ImageRef`, parses the alt-pipe width hint, resolves
//! the URL against the configured `[markdown].images_dir`, and emits
//! one [`continuity_render::InlineImagePlacement`] per inline image
//! reference. The painter consumes the resulting slice; URLs that
//! fail to resolve (broken reference, missing file) are *not*
//! emitted — they paint as the original `![](url)` text instead.
//!
//! Thread ownership: UI thread (called inside `Window::on_paint`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use continuity_buffer::BufferId;
use continuity_decorate::decorations::Decorations;
use continuity_decorate::image_link::{is_shared_store_reference, parse_image_alt};
use continuity_decorate::inline::InlineKind;
use continuity_display_map::{
    compute_image_row_reservations, ImageRowReservation, ImageRowReservationInput, SourceLine,
};
use continuity_render::{FrameDisplay, InlineImagePlacement};
use ropey::Rope;

/// Build the per-frame list of inline image placements.
///
/// * `decorations` — the focused pane's decoration snapshot. `None`
///   when no decorations have been computed yet → empty placement
///   list.
/// * `rope` — the current rope; needed to read alt + url text out of
///   each `ImageRef`'s byte ranges and to map the byte offset to a
///   source line.
/// * `images_dir` — the resolved
///   [`continuity_config::MarkdownConfig::images_dir`] absolute
///   path. `None` when settings have not loaded yet → only emit
///   placements whose URL is already absolute.
/// * `inline_images_enabled` — the `[markdown].inline_images` toggle.
///   `false` ⇒ empty list (renderer skips its paint pass).
#[must_use]
pub(crate) fn build_inline_image_placements(
    decorations: Option<&Decorations>,
    rope: &Rope,
    images_dir: Option<&Path>,
    inline_images_enabled: bool,
    buffer_id: BufferId,
    expand_state: &HashMap<(BufferId, usize), bool>,
    frame_display: &FrameDisplay,
) -> Vec<InlineImagePlacement> {
    if !inline_images_enabled {
        return Vec::new();
    }
    let Some(decorations) = decorations else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for span in &decorations.inlines {
        let InlineKind::ImageRef {
            alt_range,
            url_range,
        } = span.kind
        else {
            continue;
        };
        let alt_text = rope_slice_to_string(rope, alt_range.start, alt_range.end);
        let url_text = rope_slice_to_string(rope, url_range.start, url_range.end);
        let attrs = parse_image_alt(&alt_text);
        let Some(path) = resolve_image_path(&url_text, images_dir) else {
            continue;
        };
        if !path.is_file() {
            // Broken reference (no on-disk file). Skipping here means
            // the renderer paints the raw `![](url)` text — exactly
            // the same fallback as a broken external URL.
            continue;
        }
        let source_line = rope_byte_to_line(rope, url_range.start);
        // Folded source line ⇒ no visible row, so skip.
        if frame_display.display_line_count_for_source(source_line) == 0 {
            continue;
        }
        let display_line = frame_display.first_display_line_index_for_source(source_line);
        let source_byte = url_range.start;
        let is_expanded = expand_state
            .get(&(buffer_id, source_byte))
            .copied()
            .unwrap_or(false);
        out.push(InlineImagePlacement {
            path,
            attrs,
            display_line,
            is_expanded,
            url: url_text,
            source_byte,
        });
    }
    out
}

/// γ — build the per-frame [`ImageRowReservationInput`] list for the
/// display-map row reservation provider.
///
/// Same gating as [`build_inline_image_placements`] — disabled toggle
/// or missing decorations short-circuit to an empty vector — but the
/// output is keyed on source-line (not display-line) because the
/// reservation provider feeds the display-map builder *before* the
/// `FrameDisplay` exists.
///
/// `native_dimensions_for` peeks the renderer's `ImageCache` for each
/// resolved image path; cold-cache paths return `None`, in which case
/// the provider emits no reservation and the source line keeps its
/// natural single display row for that frame (the existing
/// pre-reservation behaviour). The first paint after expand decodes
/// the image, the next paint flips the cache warm and the reservation
/// kicks in.
#[must_use]
pub(crate) fn build_image_row_reservation_inputs(
    decorations: Option<&Decorations>,
    rope: &Rope,
    images_dir: Option<&Path>,
    inline_images_enabled: bool,
    buffer_id: BufferId,
    expand_state: &HashMap<(BufferId, usize), bool>,
    native_dimensions_for: &mut dyn FnMut(&Path) -> Option<(u32, u32)>,
) -> Vec<ImageRowReservationInput> {
    if !inline_images_enabled {
        return Vec::new();
    }
    let Some(decorations) = decorations else {
        return Vec::new();
    };
    let mut out: Vec<ImageRowReservationInput> = Vec::new();
    for span in &decorations.inlines {
        let InlineKind::ImageRef {
            alt_range,
            url_range,
        } = span.kind
        else {
            continue;
        };
        let alt_text = rope_slice_to_string(rope, alt_range.start, alt_range.end);
        let url_text = rope_slice_to_string(rope, url_range.start, url_range.end);
        let attrs = parse_image_alt(&alt_text);
        let Some(path) = resolve_image_path(&url_text, images_dir) else {
            continue;
        };
        if !path.is_file() {
            continue;
        }
        let source_line = rope_byte_to_line(rope, url_range.start);
        let source_byte = url_range.start;
        let is_expanded = expand_state
            .get(&(buffer_id, source_byte))
            .copied()
            .unwrap_or(false);
        let native_dimensions = if is_expanded {
            native_dimensions_for(&path)
        } else {
            None
        };
        out.push(ImageRowReservationInput {
            source_line: SourceLine(source_line as u32),
            is_expanded,
            native_dimensions,
            width_hint: attrs.width,
        });
    }
    out
}

/// γ — full per-frame pipeline: build reservation inputs and run the
/// row-reservation provider. Combines the two calls so the paint
/// orchestrator stays compact (one line per pane).
#[must_use]
#[allow(clippy::too_many_arguments)] // every arg is a distinct per-frame paint input
pub(crate) fn compute_image_reservations_for_pane(
    decorations: Option<&Decorations>,
    rope: &Rope,
    images_dir: Option<&Path>,
    inline_images_enabled: bool,
    buffer_id: BufferId,
    expand_state: &HashMap<(BufferId, usize), bool>,
    cached_image_dimensions: &mut dyn FnMut(&Path) -> Option<(u32, u32)>,
    line_height_dip: f32,
    pane_width_dip: f32,
) -> Vec<ImageRowReservation> {
    let inputs = build_image_row_reservation_inputs(
        decorations,
        rope,
        images_dir,
        inline_images_enabled,
        buffer_id,
        expand_state,
        cached_image_dimensions,
    );
    if inputs.is_empty() {
        return Vec::new();
    }
    compute_image_row_reservations(&inputs, line_height_dip, pane_width_dip)
}

/// Phase F — merge the image-row reservations with the table-row
/// reservations derived from `table_layouts`, taking the max reserved
/// rows per source line and returning the set sorted ascending (the
/// order the display-map row-count walker steps through with its
/// monotonic cursor). When no table needs extra rows the image set is
/// returned untouched.
#[must_use]
pub(crate) fn merge_table_row_reservations(
    image_reservations: Vec<ImageRowReservation>,
    table_layouts: &[continuity_render::TableLayout],
) -> Vec<ImageRowReservation> {
    let table = continuity_render::table_row_reservations(table_layouts);
    if table.is_empty() {
        return image_reservations;
    }
    let mut by_line: std::collections::BTreeMap<u32, u32> = std::collections::BTreeMap::new();
    for reservation in image_reservations.into_iter().chain(table) {
        let slot = by_line.entry(reservation.source_line.raw()).or_insert(0);
        *slot = (*slot).max(reservation.reserved_display_rows);
    }
    by_line
        .into_iter()
        .map(|(line, rows)| ImageRowReservation {
            source_line: SourceLine(line),
            reserved_display_rows: rows,
        })
        .collect()
}

fn rope_slice_to_string(rope: &Rope, start: usize, end: usize) -> String {
    let len = rope.len_bytes();
    let s = start.min(len);
    let e = end.min(len);
    if s >= e {
        return String::new();
    }
    // Decoration spans (e.g. image alt/url ranges) can lag the rope
    // by one or more revisions when the worker re-parse hasn't
    // caught up to a recent edit. Slicing across a misaligned
    // multi-byte UTF-8 boundary panics; treat misalignment as
    // "skip this span this frame" — the renderer paints the raw
    // source bytes and the next decoration delivery re-aligns the
    // ranges. Matches the defensive guard in
    // `crates/render/src/table_layout/build.rs`.
    if rope.try_byte_to_char(s).is_err() || rope.try_byte_to_char(e).is_err() {
        return String::new();
    }
    rope.byte_slice(s..e).to_string()
}

fn rope_byte_to_line(rope: &Rope, byte: usize) -> usize {
    let clamped = byte.min(rope.len_bytes());
    rope.byte_to_line(clamped)
}

fn resolve_image_path(url: &str, images_dir: Option<&Path>) -> Option<PathBuf> {
    if is_shared_store_reference(url) {
        let dir = images_dir?;
        let filename = url.replace('\\', "/").strip_prefix("images/")?.to_string();
        if filename.is_empty() {
            return None;
        }
        return Some(dir.join(filename));
    }
    // External / absolute URL: only honour `file://`-style local
    // paths. http(s) URLs are out of scope for F5; they paint as
    // plain text references until a future networked fetch lands.
    if let Some(stripped) = url.strip_prefix("file:///") {
        return Some(PathBuf::from(stripped.replace('/', "\\")));
    }
    let path = PathBuf::from(url);
    if path.is_absolute() {
        return Some(path);
    }
    None
}

impl crate::Window {
    /// γ — convenience wrapper around
    /// [`compute_image_reservations_for_pane`] for the focused pane,
    /// pulling every input from `&self`. Returns an empty vector when
    /// the renderer is not yet ready or no expanded image has its
    /// dimensions cached.
    pub(crate) fn compute_focused_pane_image_reservations(
        &self,
        decorations: Option<&Decorations>,
        rope: &Rope,
        line_height_dip: f32,
        pane_width_dip: f32,
    ) -> Vec<ImageRowReservation> {
        let Some(renderer) = self.renderer.as_ref() else {
            return Vec::new();
        };
        compute_image_reservations_for_pane(
            decorations,
            rope,
            self.image_store_dir.as_deref(),
            self.inline_images_enabled,
            self.buffer_id,
            &self.image_expand_state,
            &mut |path| renderer.cached_image_dimensions(path),
            line_height_dip,
            pane_width_dip,
        )
    }

    /// Toggle the expand state for the image occurrence at
    /// `source_byte` in the focused buffer. Used by the mouse
    /// hit-test path and (future) by the `image.toggle_expand` palette
    /// command. Keying by byte offset rather than URL makes two
    /// references to the same image in one buffer toggle
    /// independently. Invalidates the window so the next paint
    /// reflects the new state.
    pub(crate) fn toggle_image_expand_at(&mut self, source_byte: usize) {
        let key = (self.buffer_id, source_byte);
        let new = !self.image_expand_state.get(&key).copied().unwrap_or(false);
        self.image_expand_state.insert(key, new);
        crate::window_helpers::invalidate_hwnd(self.hwnd);
    }

    /// Try to handle a click at pane-body-relative `(x_pane, y_pane)`
    /// as a tap on a collapsed-image affordance. Returns `true` when
    /// a hit fired (caller should swallow the click). Reads from the
    /// renderer's last-frame hit cache.
    pub(crate) fn try_image_hit_at(&mut self, x_pane: f32, y_pane: f32) -> bool {
        let source_byte = {
            let Some(renderer) = self.renderer.as_ref() else {
                return false;
            };
            let hits = renderer.image_hits();
            hits.iter().rev().find_map(|h| {
                let (hx, hy, hw, hh) = h.rect;
                if x_pane >= hx && x_pane < hx + hw && y_pane >= hy && y_pane < hy + hh {
                    Some(h.source_byte)
                } else {
                    None
                }
            })
        };
        match source_byte {
            Some(b) => {
                self.toggle_image_expand_at(b);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;

    #[test]
    fn shared_store_reference_resolves_against_images_dir() {
        let images = Path::new("C:/users/example/AppData/continuity/images");
        let resolved = resolve_image_path("images/abc123.png", Some(images)).expect("resolve");
        assert!(resolved.ends_with("abc123.png"));
        assert!(resolved.to_string_lossy().contains("continuity"));
    }

    #[test]
    fn shared_store_reference_accepts_backslash() {
        let images = Path::new("C:/store");
        let resolved = resolve_image_path("images\\abc.png", Some(images)).expect("resolve");
        assert!(resolved.ends_with("abc.png"));
    }

    #[test]
    fn missing_images_dir_drops_shared_store_reference() {
        assert!(resolve_image_path("images/abc.png", None).is_none());
    }

    #[test]
    fn absolute_external_path_passes_through() {
        let resolved = resolve_image_path("D:/photos/cat.png", None);
        assert_eq!(resolved, Some(PathBuf::from("D:/photos/cat.png")));
    }

    #[test]
    fn http_url_is_not_resolved() {
        assert!(resolve_image_path("https://example.com/x.png", None).is_none());
    }

    fn test_buffer_id() -> BufferId {
        BufferId::new()
    }

    fn frame_display_for(rope: &Rope, decorations: Option<&Decorations>) -> FrameDisplay {
        FrameDisplay::build(rope, 1, decorations, &[], 0, 8.0)
    }

    #[test]
    fn empty_decorations_yield_empty_placements() {
        let rope = Rope::from_str("hello");
        let state: HashMap<(BufferId, usize), bool> = HashMap::new();
        let fd = frame_display_for(&rope, None);
        let placements =
            build_inline_image_placements(None, &rope, None, true, test_buffer_id(), &state, &fd);
        assert!(placements.is_empty());
    }

    #[test]
    fn disabled_toggle_yields_empty_placements_even_with_decorations() {
        let rope = Rope::from_str("![](images/x.png)");
        let decorations = Decorations::compute(&rope.to_string(), 1).expect("decorations");
        let state: HashMap<(BufferId, usize), bool> = HashMap::new();
        let fd = frame_display_for(&rope, Some(&decorations));
        let placements = build_inline_image_placements(
            Some(&decorations),
            &rope,
            Some(Path::new("nonexistent")),
            false,
            test_buffer_id(),
            &state,
            &fd,
        );
        assert!(placements.is_empty());
    }

    #[test]
    fn expand_state_map_default_is_collapsed() {
        // The unit asserts the precise contract: an *absent* entry
        // and a `false` entry both collapse; only `true` expands.
        let mut state: HashMap<(BufferId, usize), bool> = HashMap::new();
        let buf = test_buffer_id();
        let source_byte: usize = 42;

        // Absent → collapsed default.
        assert!(!state.get(&(buf, source_byte)).copied().unwrap_or(false));

        // Explicit false → still collapsed.
        state.insert((buf, source_byte), false);
        assert!(!state.get(&(buf, source_byte)).copied().unwrap_or(false));

        // Explicit true → expanded.
        state.insert((buf, source_byte), true);
        assert!(state.get(&(buf, source_byte)).copied().unwrap_or(false));
    }

    #[test]
    fn expand_state_is_per_occurrence() {
        // Two occurrences of the same URL at different source byte
        // offsets must not share expand state. Two different buffers
        // are independent even at the same offset.
        let buf_a = test_buffer_id();
        let buf_b = test_buffer_id();
        assert_ne!(buf_a, buf_b);
        let mut state: HashMap<(BufferId, usize), bool> = HashMap::new();
        state.insert((buf_a, 10), true);
        assert_eq!(state.get(&(buf_a, 10)).copied(), Some(true));
        // Same buffer, different occurrence → independent.
        assert!(!state.contains_key(&(buf_a, 200)));
        // Different buffer, same offset → independent.
        assert!(!state.contains_key(&(buf_b, 10)));
    }
}
