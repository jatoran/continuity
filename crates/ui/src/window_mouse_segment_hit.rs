//! Phase 17.6 cleanup tail #5: `SegmentHit`-driven click handling.
//!
//! Ctrl+click on a link segment opens the URL via [`ShellExecuteW`]; a
//! plain click on a checkbox segment flips the toggle source byte
//! through the existing `markdown.toggle_checkbox` command. Both paths
//! resolve the click position through the active pane's
//! [`continuity_render::FrameDisplay`] and consult the segment under
//! the cursor — replacing the legacy per-frame `link_hit_ranges` /
//! `checkbox_hit_ranges` storage that Phase 17.6 retired.

use continuity_decorate::{Decorations, InlineKind};
use continuity_display_map::{
    compute_line_projection_stamp, DisplaySegment, SegmentCacheKey, SegmentHit, SourceByte,
};
use continuity_text::{Position, Selection};
use windows::core::PCWSTR;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::Window;

/// `MK_CONTROL` bit from `wParam` of `WM_LBUTTONDOWN` — set while the
/// Ctrl key is held during the click.
pub(crate) const MK_CONTROL: u32 = 0x0008;

/// P0.7.1 — maximum time the segment-hit cold-build branch is allowed
/// to spend resolving a fresh [`FrameDisplay`] before the click falls
/// back to a coarse row-only caret placement. The 500 µs budget is
/// derived from the WM_LBUTTONDOWN budget of 1 ms minus the upstream
/// `client_to_buffer_position` cost (warm-cache p99 < 500 µs in
/// `perf-snapshots/trace_20260520-102959.report.md`); overrun means
/// caches missed and a 6 – 10 ms stall would otherwise land on the
/// user's first post-edit click.
const SEGMENT_HIT_COLD_BUDGET_US: u64 = 500;

impl Window {
    /// Resolve `(x, y)` through the active pane's display projection and
    /// react to any `SegmentHit` carried by the segment under the click.
    /// Returns `true` when the click was consumed.
    pub(crate) fn try_handle_segment_hit(&mut self, x: i32, y: i32, key_state: u32) -> bool {
        let Some(hit) = self.segment_hit_at_client(x, y) else {
            return false;
        };
        self.dispatch_segment_hit(&hit, key_state)
    }

    /// Does the editor body at `(x, y)` sit over a [`SegmentHit::Link`]?
    /// Used by `on_set_cursor` to swap to `IDC_HAND` when Ctrl is held.
    /// Returns `false` cheaply for points outside the editor body or any
    /// line, so the cost is paid only on actual mouse moves over text.
    pub(crate) fn cursor_over_ctrl_click_target(&self, x: i32, y: i32) -> bool {
        matches!(
            self.segment_hit_at_client(x, y),
            Some(SegmentHit::Link { .. })
                | Some(SegmentHit::FootnoteReference {
                    definition: Some(_),
                    ..
                })
                | Some(SegmentHit::FootnoteDefinition {
                    first_reference: Some(_),
                    ..
                })
        )
    }

    /// Does the editor body at `(x, y)` sit over a rendered checkbox
    /// glyph? Used by `on_set_cursor` to swap the text I-beam for the
    /// default arrow so a task checkbox reads as a clickable target
    /// rather than editable text. Only a *replaced* (unrevealed) checkbox
    /// produces [`SegmentHit::Checkbox`]; once the caret reveals the raw
    /// `[ ]` brackets they are editable text and keep the I-beam.
    pub(crate) fn cursor_over_checkbox(&self, x: i32, y: i32) -> bool {
        matches!(
            self.segment_hit_at_client(x, y),
            Some(SegmentHit::Checkbox { .. })
        )
    }

    /// Does the editor body at `(x, y)` sit over a collapsed link that a
    /// plain click would open? Used by `on_set_cursor` to show the hand
    /// cursor (no Ctrl needed) so rendered links read as clickable.
    pub(crate) fn cursor_over_open_link(&self, x: i32, y: i32) -> bool {
        matches!(
            self.segment_hit_at_client(x, y),
            Some(SegmentHit::Link { .. })
        )
    }

    fn segment_hit_at_client(&self, x: i32, y: i32) -> Option<SegmentHit> {
        let pos = self.client_to_buffer_position(x, y)?;
        let snap = self.editor.snapshot(self.buffer_id)?;
        let rope = snap.rope_snapshot().rope();
        let line_idx = pos.line as usize;
        let line_start_byte = if line_idx < rope.len_lines() {
            rope.line_to_byte(line_idx)
        } else {
            rope.len_bytes()
        };
        let click_abs_src = SourceByte::from_usize(line_start_byte + pos.byte_in_line as usize);
        let revision = snap.rope_snapshot().revision().0;
        let decorations = self
            .decoration_cache
            .get(self.buffer_id.as_uuid().as_u128());
        let caret_bytes = Self::caret_bytes_for_projection(rope, snap.selections());
        let metrics =
            self.display_projection_metrics(self.current_search_minimap_active(), rope.len_lines());
        let (_query, folds) = self.hit_test_projection_query_and_folds(
            rope,
            revision,
            decorations,
            &caret_bytes,
            metrics.wrap_width_dip,
        );
        let (line_end_byte, line_text) =
            source_line_text_for_segment_cache(rope, line_idx, line_start_byte);
        let owned_empty = Decorations::empty(revision);
        let decorations_for_stamp = decorations.unwrap_or(&owned_empty);
        let caret_source_bytes: Vec<SourceByte> = caret_bytes
            .iter()
            .copied()
            .map(SourceByte::from_usize)
            .collect();
        let suppressed_table_blocks = self.compute_suppressed_table_blocks();
        let stamp = compute_line_projection_stamp(
            decorations_for_stamp,
            &caret_source_bytes,
            &folds,
            &suppressed_table_blocks,
            self.markdown_render_toggles(),
            line_start_byte,
            line_end_byte,
            &line_text,
        );
        let key = SegmentCacheKey::new(stamp, self.font_state.0);
        // P0.7.1 item 1 — consult the per-source-line segment cache
        // before any fresh layout. Applies to both ASCII and non-ASCII
        // lines: the key is content-stamp based, so the cache returns
        // matching segments at any absolute offset and bypasses the
        // FrameDisplay cold build entirely.
        if let Some(segments) = self.walker_segment_cache.get_shifted(&key, line_start_byte) {
            log_segment_hit_cache_path("segment_cache");
            return segments
                .iter()
                .find_map(|segment| segment_hit_for_click(segment, click_abs_src));
        }
        // P0.7.1 item 2 — ASCII fast-path. A pure-ASCII source line
        // cannot host RTL, combining marks, or emoji ZWJ runs, so the
        // only way a click on such a line yields a [`SegmentHit`] is
        // when a markdown inline span (Link, FootnoteReference,
        // FootnoteDefinition, or Checkbox) intersects the line's byte
        // range. When none do, the click is provably hit-free and we
        // return `None` without spending the cold-build budget below.
        if line_text.is_ascii()
            && !has_hit_producing_inline_in_line(
                decorations_for_stamp,
                line_start_byte,
                line_end_byte,
            )
        {
            log_segment_hit_cache_path("ascii_fastpath");
            return None;
        }
        // P0.7.1 item 3 — cold-build fallback bounded by
        // [`SEGMENT_HIT_COLD_BUDGET_US`]. The resolver reuses any
        // painted / spectator / mouse-hit-test frame already in cache;
        // overrun signals every cache missed and a fresh viewport
        // build ran. On overrun we emit `event:segment_hit_cold_timeout`
        // and return `None` so the caller falls through to
        // `place_caret_at_pixel` (off by at most one column on the
        // correct line) instead of charging the user a 6 – 10 ms
        // stall for a maybe-correct link / checkbox hit.
        let started = std::time::Instant::now();
        let (frame_display, _source, _folds) = self.resolve_hit_test_frame_display(
            rope,
            revision,
            decorations,
            &caret_bytes,
            metrics.wrap_width_dip,
            metrics.char_width_dip,
            self.display_row_for_client_y(y),
        );
        let total_rows = frame_display.display_line_count() as i64;
        let result = if total_rows == 0 {
            None
        } else {
            let display_row = self.display_row_for_client_y(y) as i64;
            let display_row = display_row.clamp(0, total_rows - 1) as u32;
            frame_display
                .display_line_by_index(display_row)
                .and_then(|spec| {
                    spec.segments
                        .iter()
                        .find_map(|seg| segment_hit_for_click(seg, click_abs_src))
                })
        };
        let elapsed_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        if elapsed_us > SEGMENT_HIT_COLD_BUDGET_US {
            if crate::paint_trace::is_trace_enabled() {
                crate::paint_trace::log_event(
                    "segment_hit_cold_timeout",
                    &format!("elapsed_us={elapsed_us} row={line_idx}"),
                );
            }
            return None;
        }
        log_segment_hit_cache_path("fresh_build");
        result
    }

    fn dispatch_segment_hit(&mut self, hit: &SegmentHit, key_state: u32) -> bool {
        let ctrl = (key_state & MK_CONTROL) != 0;
        match hit {
            // A collapsed link opens on a plain click (the display map
            // only attaches this hit when the link is rendered, not while
            // the caret reveals the raw `[text](url)` for editing).
            SegmentHit::Link { url } => {
                self.snapshot_open_url_at_range(url.start.raw(), url.end.raw())
            }
            SegmentHit::Checkbox { toggle, .. } if !ctrl => {
                self.toggle_checkbox_at_byte(toggle.raw())
            }
            SegmentHit::FootnoteReference {
                definition: Some(range),
                ..
            } if ctrl => self.jump_to_source_range(range),
            SegmentHit::FootnoteDefinition {
                first_reference: Some(range),
                ..
            } if ctrl => self.jump_to_source_range(range),
            _ => false,
        }
    }

    fn snapshot_open_url_at_range(&mut self, url_start: u32, url_end: u32) -> bool {
        let snap = match self.editor.snapshot(self.buffer_id) {
            Some(s) => s,
            None => return false,
        };
        let rope = snap.rope_snapshot().rope();
        let total = rope.len_bytes() as u32;
        let start = url_start.min(total) as usize;
        let end = url_end.min(total) as usize;
        if end <= start {
            return false;
        }
        let raw_url: String = rope.byte_slice(start..end).to_string();
        // In-document anchors (`#some-heading`) jump to the matching
        // heading instead of leaving the editor.
        if let Some(anchor) = raw_url.trim().strip_prefix('#') {
            return self.jump_to_heading_anchor(anchor);
        }
        // Scheme-less targets (`www.google.com`) open as files otherwise.
        let url = crate::window_link_clipboard::normalize_url_for_open(&raw_url);
        if url.is_empty() {
            return false;
        }
        let wide: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
        let verb: Vec<u16> = "open\0".encode_utf16().collect();
        unsafe {
            // Return value is HINSTANCE; >32 indicates success per Win32
            // docs. We don't surface failure to the user — silent no-op
            // matches Notepad / VS Code behaviour on broken URLs.
            let _ = ShellExecuteW(
                None,
                PCWSTR(verb.as_ptr()),
                PCWSTR(wide.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            );
        }
        true
    }

    /// `[text](#some-heading)` — resolve the anchor against the
    /// buffer's ATX headings (GitHub-style slug comparison, lenient
    /// about punctuation and `%20`s) and jump the caret there.
    fn jump_to_heading_anchor(&mut self, anchor: &str) -> bool {
        let Some(snap) = self.editor.snapshot(self.buffer_id) else {
            return false;
        };
        let rope = snap.rope_snapshot().rope();
        let wanted = heading_anchor_slug(&anchor.replace("%20", " "));
        if wanted.is_empty() {
            return false;
        }
        for line in 0..rope.len_lines() {
            let start = rope.line_to_byte(line);
            let end = if line + 1 < rope.len_lines() {
                rope.line_to_byte(line + 1)
            } else {
                rope.len_bytes()
            };
            let text = rope.byte_slice(start..end).to_string();
            let trimmed = text.trim_end();
            let hashes = trimmed.bytes().take_while(|b| *b == b'#').count();
            if hashes == 0 || hashes > 6 {
                continue;
            }
            let Some(title) = trimmed[hashes..].strip_prefix(' ') else {
                continue;
            };
            if heading_anchor_slug(title) == wanted {
                let s = SourceByte::from_usize(start);
                return self.jump_to_source_range(&(s..s));
            }
        }
        false
    }

    fn toggle_checkbox_at_byte(&mut self, abs_byte: u32) -> bool {
        let snap = match self.editor.snapshot(self.buffer_id) {
            Some(s) => s,
            None => return false,
        };
        let rope = snap.rope_snapshot().rope();
        let byte = abs_byte as usize;
        if byte >= rope.len_bytes() {
            return false;
        }
        let line = rope.byte_to_line(byte);
        let line_start = rope.line_to_byte(line);
        let byte_in_line = (byte - line_start) as u32;
        let caret = Selection::caret_at(Position::new(line as u32, byte_in_line));
        let _ = self.editor.set_selections(self.buffer_id, vec![caret]);
        self.dispatch_command("markdown.toggle_checkbox", &serde_json::Value::Null)
    }

    fn jump_to_source_range(&mut self, range: &std::ops::Range<SourceByte>) -> bool {
        let snap = match self.editor.snapshot(self.buffer_id) {
            Some(s) => s,
            None => return false,
        };
        let rope = snap.rope_snapshot().rope();
        let byte = range.start.as_usize().min(rope.len_bytes());
        let pos = Position::from_byte_offset(rope, byte).unwrap_or(Position::ZERO);
        let caret = Selection::caret_at(pos);
        let _ = self.editor.set_selections(self.buffer_id, vec![caret]);
        self.note_input_now();
        self.ensure_primary_caret_visible();
        true
    }
}

/// Test one display segment against a click position. Pulled out so
/// the painted-frame fast path and the cold-build fallback in
/// [`Window::segment_hit_at_client`] share one matcher.
fn segment_hit_for_click(seg: &DisplaySegment, click_abs_src: SourceByte) -> Option<SegmentHit> {
    let range = seg.source_range();
    if click_abs_src < range.start || click_abs_src >= range.end {
        return None;
    }
    match seg {
        DisplaySegment::Visible { hit, .. } | DisplaySegment::Replace { hit, .. } => {
            Some(hit.clone())
        }
        DisplaySegment::Hidden { .. } => None,
    }
}

/// `true` when at least one inline decoration whose [`InlineKind`]
/// can produce a non-`None` [`SegmentHit`] intersects the source line
/// `[line_start, line_end)`. Used by the ASCII fast-path in
/// [`Window::segment_hit_at_client`] to short-circuit clicks on plain
/// text lines (no link, no checkbox, no footnote anchor) without
/// paying for a cold [`FrameDisplay`] build.
fn has_hit_producing_inline_in_line(
    decorations: &Decorations,
    line_start: usize,
    line_end: usize,
) -> bool {
    decorations.inlines.iter().any(|span| {
        if span.range.end <= line_start || span.range.start >= line_end {
            return false;
        }
        matches!(
            span.kind,
            InlineKind::Link { .. }
                | InlineKind::Checkbox { .. }
                | InlineKind::FootnoteReference { .. }
                | InlineKind::FootnoteDefinition { .. }
        )
    })
}

/// Emit the per-click `cache_path` attribution that P0.7.1 added on
/// top of the existing `click_try_handle_segment_hit` scope. No-op
/// when tracing is off — the call site does not gate this internally
/// because [`paint_trace::log_event`] already short-circuits.
fn log_segment_hit_cache_path(cache_path: &str) {
    if crate::paint_trace::is_trace_enabled() {
        crate::paint_trace::log_event(
            "segment_hit_cache_path",
            &format!("cache_path={cache_path}"),
        );
    }
}

/// GitHub-style heading slug, lenient: lowercase, alphanumerics kept,
/// whitespace and dashes collapse to single dashes, underscores kept,
/// all other punctuation dropped. Both the anchor and the heading
/// title pass through this, so `[x](#My-Header!)` matches `## My
/// Header!`.
fn heading_anchor_slug(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_dash = true; // suppress leading dashes
    for c in text.trim().chars() {
        if c.is_alphanumeric() || c == '_' {
            out.extend(c.to_lowercase());
            prev_dash = false;
        } else if (c.is_whitespace() || c == '-') && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

fn source_line_text_for_segment_cache(
    rope: &ropey::Rope,
    source_line: usize,
    line_start: usize,
) -> (usize, String) {
    let total_lines = rope.len_lines();
    let next = if source_line + 1 < total_lines {
        rope.line_to_byte(source_line + 1)
    } else {
        rope.len_bytes()
    };
    let slice = rope.byte_slice(line_start..next).to_string();
    let line_end = if slice.ends_with("\r\n") {
        next.saturating_sub(2)
    } else if slice.ends_with('\n') {
        next.saturating_sub(1)
    } else {
        next
    };
    if line_end == next {
        (line_end, slice)
    } else {
        (line_end, rope.byte_slice(line_start..line_end).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::heading_anchor_slug;

    #[test]
    fn slug_matches_github_style_anchors() {
        assert_eq!(heading_anchor_slug("My Header"), "my-header");
        assert_eq!(heading_anchor_slug("my-header"), "my-header");
        assert_eq!(heading_anchor_slug("My  Header!"), "my-header");
        assert_eq!(heading_anchor_slug("With_Underscore"), "with_underscore");
        assert_eq!(heading_anchor_slug("  Trim Me  "), "trim-me");
        assert_eq!(heading_anchor_slug("Émigré Café"), "émigré-café");
        assert_eq!(heading_anchor_slug("!!!"), "");
    }
}
