//! Search-pipeline helpers for [`crate::Window`]: recompute find matches
//! against the current buffer, jump the editor caret to the active match,
//! populate palette / quick-open / goto-heading candidate lists, and apply
//! find-bar replace operations.
//!
//! Split from `window_overlays.rs` to keep both files under the 600-line cap.

use continuity_command::{Context, Error, FindContext, Registry};
use continuity_decorate::{block_spans, MarkdownParser};
use continuity_search::find_match_ranges_dispatch;
use continuity_text::Position;

use crate::find_in_all::FlatRow;
use crate::palette::PaletteEntry;
use crate::quick_open::QuickOpenEntry;
use crate::Window;

/// P7 — emit one `event:find_pattern` trace line per find-bar
/// recompute. Records which engine ran (literal vs regex), how long
/// the dispatcher took, the match count, and the user query length.
/// Cheap no-op when tracing is disabled.
pub(crate) fn emit_find_pattern_trace(
    path: &str,
    elapsed_us: u64,
    matches: usize,
    pattern_len: usize,
) {
    if !crate::paint_trace::is_trace_enabled() {
        return;
    }
    let detail =
        format!("path={path} elapsed_us={elapsed_us} matches={matches} pattern_len={pattern_len}");
    crate::paint_trace::log_event("find_pattern", &detail);
}

impl Window {
    /// Open or retarget the find bar using the mode requested by the
    /// command while preserving cached query / replacement text.
    pub(crate) fn open_find_impl(&mut self, with_replace: bool) -> Result<(), Error> {
        if self.overlays.find_bar().is_some() {
            let ranges = self.current_find_selection_ranges();
            if let Some(fb) = self.overlays.find_bar_mut() {
                fb.apply_requested_find_mode(with_replace);
                if fb.selection_scope_ranges.is_empty() {
                    fb.selection_scope_ranges = ranges;
                }
            }
            self.focus_overlay_input();
            self.ensure_find_matches_current_for_focused_pane();
            self.save_find_memento();
            return Ok(());
        }
        let mut fb = self
            .current_find_memento()
            .as_ref()
            .map(crate::find_bar::FindBar::from_memento)
            .unwrap_or_default();
        fb.apply_requested_find_mode(with_replace);
        fb.selection_scope_ranges = self.current_find_selection_ranges();
        self.overlays.open_find_with(fb);
        self.focus_overlay_input();
        self.recompute_find_matches();
        Ok(())
    }

    /// Re-run the in-buffer find against the current snapshot.
    pub(crate) fn recompute_find_matches(&mut self) {
        self.recompute_find_matches_impl(true, true);
    }

    /// Shared recompute path; callers choose whether it jumps or persists mementos.
    pub(crate) fn recompute_find_matches_impl(&mut self, should_jump: bool, should_save: bool) {
        let Some(snap) = self.editor.snapshot(self.buffer_id) else {
            return;
        };
        let revision = snap.rope_snapshot().revision().0;
        let target = self.current_find_target(revision);
        let target_label = self.current_find_target_label();
        let text = snap.rope_snapshot().rope().to_string();
        let Some(fb) = self.overlays.find_bar_mut() else {
            return;
        };
        fb.target_label = target_label;
        if fb.query().is_empty() {
            fb.set_results_for_target(Vec::new(), target);
            fb.regex_error = None;
            return;
        }
        let query_owned = fb.query().to_owned();
        let case_sensitive = fb.case_sensitive;
        let whole_word = fb.whole_word;
        let is_regex = fb.regex;
        let scope = fb.scope;
        let selected_ranges = if scope == crate::find_bar::FindScope::Selection {
            Some(fb.selection_scope_ranges.clone())
        } else {
            None
        };
        let pattern_len = query_owned.len();
        match find_match_ranges_dispatch(&query_owned, &text, is_regex, case_sensitive, whole_word)
        {
            Ok(result) => {
                let path = result.path.as_trace_label();
                let elapsed_us = result.elapsed_us;
                let mut matches = result.matches;
                if let Some(ranges) = selected_ranges.as_deref() {
                    crate::find_scope::retain_matches_in_ranges(&mut matches, ranges);
                }
                emit_find_pattern_trace(path, elapsed_us, matches.len(), pattern_len);
                // δ.3 — clear any stale regex-error so the X-of-N
                // counter reverts to the normal "match N of M"
                // display once the query parses again.
                fb.regex_error = None;
                fb.set_results_for_target(matches, target);
            }
            Err(continuity_search::Error::InvalidRegex(msg)) => {
                fb.regex_error = Some(format!("invalid regex: {msg}"));
                fb.set_results_for_target(Vec::new(), target);
            }
            Err(_) => {
                fb.regex_error = None;
                fb.set_results_for_target(Vec::new(), target);
            }
        }
        // G2: persist the user-input state after every query change so a
        // re-open in this buffer brings the bar back populated.
        if should_save {
            self.save_find_memento();
        }
        if should_jump {
            self.jump_to_current_find_match();
        }
    }

    /// Move the editor's primary selection to the highlighted find match.
    pub(crate) fn jump_to_current_find_match(&mut self) {
        if !self.find_matches_are_current_for_focused_pane() {
            return;
        }
        let Some(fb) = self.overlays.find_bar() else {
            return;
        };
        let Some(m) = fb.current_match() else {
            return;
        };
        let Some(snap) = self.editor.snapshot(self.buffer_id) else {
            return;
        };
        let rope = snap.rope_snapshot().rope().clone();
        let Ok(start) = Position::from_byte_offset(&rope, m.start_byte) else {
            return;
        };
        let Ok(end) = Position::from_byte_offset(&rope, m.end_byte) else {
            return;
        };
        let from_line = self.capture_caret_line_for_jump();
        let _ = self.editor.set_selections(
            self.buffer_id,
            vec![continuity_text::Selection::new(
                start,
                end,
                continuity_text::SelectionKind::Caret,
            )],
        );
        // Centre the match in the viewport so the user sees context
        // around it. F3 / Enter in the find bar bypass `dispatch_command`
        // (see `window_overlays.rs` VK_F3 / VK_RETURN handlers), so the
        // default `ensure_primary_caret_visible` post-hook never runs —
        // without this call the caret moves but the viewport doesn't
        // follow, and matches outside the current view stay invisible.
        self.center_primary_caret_in_viewport();
        // Phase B6: acknowledge a long find-jump with a brief glow.
        self.maybe_trigger_jump_glow(from_line);
        // Phase B7: tween the caret to the find target as part of the
        // same edit-driven motion.
        if let Some(f) = from_line {
            self.maybe_start_caret_tween(f);
        }
    }

    /// Re-run the find-in-all-buffers query.
    pub(crate) fn recompute_find_in_all(&mut self) {
        let buffers = self.editor.list_buffers();
        let Some(fia) = self.overlays.find_in_all_mut() else {
            return;
        };
        if fia.input.text.is_empty() {
            fia.set_rows(Vec::new());
            return;
        }
        let query_owned = fia.input.text.clone();
        let case_sensitive = fia.case_sensitive;
        let whole_word = fia.whole_word;
        let is_regex = fia.regex;
        let pattern_len = query_owned.len();
        let started = std::time::Instant::now();
        let mut path_taken: Option<&'static str> = None;
        let mut total_matches: usize = 0;
        let mut rows: Vec<FlatRow> = Vec::new();
        for summary in &buffers {
            let Some(snap) = self.editor.snapshot(summary.id) else {
                continue;
            };
            let text = snap.rope_snapshot().rope().to_string();
            let Ok(result) = find_match_ranges_dispatch(
                &query_owned,
                &text,
                is_regex,
                case_sensitive,
                whole_word,
            ) else {
                continue;
            };
            // All buffers route through the same classifier, so the
            // first non-empty result's path label applies to the
            // batch. Cache it for the single aggregate trace event
            // emitted at the end of the loop.
            if path_taken.is_none() {
                path_taken = Some(result.path.as_trace_label());
            }
            total_matches += result.matches.len();
            for m in result.matches {
                let line_text = text
                    .lines()
                    .nth((m.line.saturating_sub(1)) as usize)
                    .unwrap_or("")
                    .to_string();
                rows.push(FlatRow {
                    buffer_id: summary.id,
                    buffer_title: summary.title.clone().unwrap_or_else(|| "Untitled".into()),
                    line: m.line,
                    start_byte: m.start_byte,
                    end_byte: m.end_byte,
                    line_text,
                });
            }
        }
        let elapsed_us = started.elapsed().as_micros() as u64;
        emit_find_pattern_trace(
            path_taken.unwrap_or("literal"),
            elapsed_us,
            total_matches,
            pattern_len,
        );
        fia.set_rows(rows);
    }

    /// Populate the palette's candidate list from the registry + active keymap.
    pub(crate) fn populate_palette_candidates(&mut self) {
        let ids: Vec<String> = Registry::ids(&self.registry)
            .map(|id| id.as_str().to_string())
            .collect();
        let mut entries: Vec<PaletteEntry> = Vec::with_capacity(ids.len());
        for cmd in ids {
            let applicable = self
                .registry
                .handler_for_name(&cmd, self as &dyn Context)
                .is_ok();
            let keybinding = self
                .keymap
                .bindings
                .iter()
                .find(|b| b.command == cmd)
                .and_then(|b| b.keys.first().map(|c| c.to_string()));
            let description = self.registry.description(&cmd).map(str::to_owned);
            entries.push(PaletteEntry {
                command: cmd,
                keybinding,
                description,
                applicable,
            });
        }
        entries.sort_by(|a, b| a.command.cmp(&b.command));
        // δ.2 — clone the window-level recency map into the fresh
        // palette so its `refilter` ordering reflects history across
        // open/dismiss cycles.
        let recency = self.palette_command_recency.clone();
        let tick = self.palette_recency_tick;
        if let Some(p) = self.overlays.palette_mut() {
            p.set_candidates(entries);
            p.last_used = recency;
            p.recency_tick = tick;
            p.refilter();
        }
    }

    /// Populate the quick-open list from the editor's open buffers.
    pub(crate) fn populate_quick_open_candidates(&mut self) {
        let buffers = self.editor.list_buffers();
        if let Some(q) = self.overlays.quick_open_mut() {
            let entries: Vec<QuickOpenEntry> = buffers
                .into_iter()
                .map(|s| QuickOpenEntry {
                    id: s.id,
                    title: s.title.unwrap_or_else(|| "Untitled".to_string()),
                    first_line: s.first_line,
                })
                .collect();
            q.set_candidates(entries);
        }
    }

    /// Populate the goto-heading list by parsing the active buffer.
    pub(crate) fn populate_goto_heading(&mut self) {
        let Some(snap) = self.editor.snapshot(self.buffer_id) else {
            return;
        };
        let text = snap.rope_snapshot().rope().to_string();
        let Ok(mut parser) = MarkdownParser::new() else {
            return;
        };
        let Some(tree) = parser.parse(&text, None) else {
            return;
        };
        let spans = block_spans(&tree);
        let entries = continuity_decorate::headings(&spans, &text);
        if let Some(g) = self.overlays.goto_heading_mut() {
            g.set_candidates(entries);
        }
    }

    /// G2: snapshot the active find bar's input state into `find_memory`.
    /// No-op when the bar is closed or `find_persist_per_buffer` is off.
    pub(crate) fn save_find_memento(&mut self) {
        if !self.find_persist_per_buffer {
            return;
        }
        let Some(fb) = self.overlays.find_bar() else {
            return;
        };
        let memento = fb.to_memento();
        // If the user never typed anything, don't bother retaining an
        // empty memento — it's the same as no memory.
        if memento.query.is_empty()
            && memento.replace.is_empty()
            && !memento.replace_visible
            && !memento.case_sensitive
            && !memento.whole_word
            && !memento.regex
            && !memento.preserve_case
            && memento.scope == crate::find_bar::FindScope::Buffer
        {
            self.find_memory.remove(&self.buffer_id);
            return;
        }
        self.find_memory.insert(self.buffer_id, memento);
    }

    /// G2: drop the find-bar memento for `buffer_id`. Called when a buffer
    /// closes so the memory map doesn't leak stale entries.
    pub(crate) fn forget_find_memory(&mut self, buffer_id: continuity_buffer::BufferId) {
        self.find_memory.remove(&buffer_id);
    }

    /// G2: clone the saved memento for the current buffer, if any.
    /// `open_find` consults this to pre-populate the new find bar.
    #[must_use]
    pub(crate) fn current_find_memento(&self) -> Option<crate::find_bar::FindBarMemento> {
        if !self.find_persist_per_buffer {
            return None;
        }
        self.find_memory.get(&self.buffer_id).cloned()
    }

    /// G3: convert every find-bar match into its own cursor (one
    /// selection per match), then dismiss the bar. No-op when no bar
    /// is open or the match set is empty.
    pub(crate) fn find_matches_to_cursors_impl(&mut self) {
        self.ensure_find_matches_current_for_focused_pane();
        let Some(fb) = self.overlays.find_bar() else {
            return;
        };
        if fb.matches.is_empty() {
            return;
        }
        let Some(snap) = self.editor.snapshot(self.buffer_id) else {
            return;
        };
        let rope = snap.rope_snapshot().rope().clone();
        let mut sels: Vec<continuity_text::Selection> = Vec::with_capacity(fb.matches.len());
        for m in &fb.matches {
            let Ok(start) = continuity_text::Position::from_byte_offset(&rope, m.start_byte) else {
                continue;
            };
            let Ok(end) = continuity_text::Position::from_byte_offset(&rope, m.end_byte) else {
                continue;
            };
            sels.push(continuity_text::Selection::new(
                start,
                end,
                continuity_text::SelectionKind::Caret,
            ));
        }
        if sels.is_empty() {
            return;
        }
        let _ = self.editor.set_selections(self.buffer_id, sels);
        self.overlays.dismiss();
    }

    /// G3: Sublime-style skip — drop the primary cursor and add a
    /// fresh one at the next occurrence of the primary selection's
    /// text. Returns `false` when there is nothing to skip.
    pub(crate) fn skip_current_match_impl(&mut self) -> bool {
        self.skip_current_match_at_selection()
    }

    /// G4: hit-test a left-click against the cached search-active
    /// minimap strip layout. On a hit, jump the find bar to the
    /// matching tick (re-using `find_step`'s match-cursor mover) and
    /// return `true` so the caller short-circuits caret placement.
    ///
    /// Returns `false` when the strip is not painted this frame, the
    /// click misses every tick within the hit slop, or the click is
    /// outside the focused pane body.
    pub(crate) fn try_search_minimap_left_down(&mut self, x: i32, y: i32) -> bool {
        self.ensure_find_matches_current_for_focused_pane();
        let Some(layout) = self.view_options.search_minimap_layout.clone() else {
            return false;
        };
        let body = self.focused_body_rect();
        let xf = x as f32;
        let yf = y as f32;
        if yf < body.y || yf >= body.y + body.h {
            return false;
        }
        // Strip coordinates are pane-local; convert client → pane.
        let x_local = xf - body.x;
        let y_local = yf - body.y;
        // Slop matches a few text rows so a fat-finger click still lands
        // on the nearest tick instead of falling through to caret-place.
        const HIT_SLOP_DIP: f32 = 6.0;
        let Some(target) = crate::search_minimap::hit_test(&layout, x_local, y_local, HIT_SLOP_DIP)
        else {
            return false;
        };
        let delta = {
            let Some(fb) = self.overlays.find_bar() else {
                return false;
            };
            if target >= fb.matches.len() {
                return false;
            }
            target as i32 - fb.current as i32
        };
        self.step_find_bar(delta);
        true
    }

    /// G1: flip one of the find-bar mode toggles and re-run the query
    /// so the X-of-N counter updates.
    pub(crate) fn find_toggle_mode_impl(&mut self, mode: &str) {
        if mode == "scope" {
            let ranges = self.current_find_selection_ranges();
            let Some(fb) = self.overlays.find_bar_mut() else {
                return;
            };
            if fb.scope == crate::find_bar::FindScope::Buffer
                && fb.selection_scope_ranges.is_empty()
            {
                fb.selection_scope_ranges = ranges;
            }
            fb.toggle_scope();
            self.save_find_memento();
            self.recompute_find_matches();
            return;
        }
        let Some(fb) = self.overlays.find_bar_mut() else {
            return;
        };
        match mode {
            "case" => fb.toggle_case_sensitive(),
            "word" => fb.toggle_whole_word(),
            "regex" => fb.toggle_regex(),
            "preserve" => fb.toggle_preserve_case(),
            _ => return,
        }
        self.save_find_memento();
        self.recompute_find_matches();
    }
}

impl FindContext for Window {
    fn find_step(&mut self, delta: i32) -> Result<(), Error> {
        self.step_find_bar(delta);
        Ok(())
    }

    fn find_replace_one(&mut self) -> Result<(), Error> {
        self.find_replace_one_impl()
    }

    fn find_replace_all(&mut self) -> Result<(), Error> {
        self.find_replace_all_impl()
    }

    fn find_toggle(&mut self, mode: &str) -> Result<(), Error> {
        self.find_toggle_mode_impl(mode);
        Ok(())
    }

    fn find_matches_to_cursors(&mut self) -> Result<(), Error> {
        self.find_matches_to_cursors_impl();
        Ok(())
    }

    fn skip_current_match(&mut self) -> Result<(), Error> {
        let _ = self.skip_current_match_impl();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::window_find_replace::replace_all_banner_text;

    #[test]
    fn replace_all_banner_zero_matches() {
        assert_eq!(replace_all_banner_text(0), "No matches to replace");
    }

    #[test]
    fn replace_all_banner_one_match_uses_singular() {
        assert_eq!(
            replace_all_banner_text(1),
            "Replaced 1 match (Ctrl+Z to undo)"
        );
    }

    #[test]
    fn replace_all_banner_many_matches_uses_plural() {
        assert_eq!(
            replace_all_banner_text(10),
            "Replaced 10 matches (Ctrl+Z to undo)"
        );
    }
}
