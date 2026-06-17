//! Tab-label resolution (Phase B15) split out of [`crate::pane_tree`]
//! to keep that file under the 600-line cap once H6's preview/peek
//! helpers + tests landed.
//!
//! No state of its own — pure functions over [`crate::pane_tree::Tab`]
//! plus a candidate first-line snippet. Spec §6 precedence: explicit
//! `label_override` → first non-empty trimmed line of the buffer
//! (leading `#` markers stripped, capped at [`MAX_BASE_TITLE_CHARS`])
//! → `Untitled`.
//!
//! Item 8 — the label is no longer hard-clipped to 20 chars *before*
//! layout. The renderer now performs ellipsis fitting against the tab's
//! actual painted slot width (`pane_chrome`), so a wide tab shows more
//! of its title and a crowded tab shows less. We still cap the base
//! title at a generous [`MAX_BASE_TITLE_CHARS`] so a pathological
//! first line (e.g. a 50 KB single line) does not balloon every
//! per-frame DirectWrite layout.

use crate::pane_tree::Tab;

/// Generous upper bound on the characters carried in a tab's base title.
/// The renderer fits an ellipsis against the real slot width below this;
/// the cap only guards against pathologically long first lines inflating
/// the per-frame text layout cost.
pub(crate) const MAX_BASE_TITLE_CHARS: usize = 60;

/// Resolve a tab's display label.
///
/// Precedence per spec §6: explicit `label_override` → first non-empty
/// trimmed line of the buffer (leading `#` heading markers stripped,
/// capped at [`MAX_BASE_TITLE_CHARS`]) → `Untitled`. Ellipsis fitting to
/// the painted slot width happens at render time, not here.
pub(crate) fn resolve_label(tab: &Tab, first_line: Option<&str>) -> String {
    let base = base_label(tab, first_line);
    if tab.pinned {
        // δ.1 — pin dot prefix. U+25CF BLACK CIRCLE renders compactly
        // in the tab strip's chrome font.
        format!("\u{25CF} {base}")
    } else {
        base
    }
}

fn base_label(tab: &Tab, first_line: Option<&str>) -> String {
    if let Some(s) = tab.label_override.as_deref() {
        if !s.is_empty() {
            return clip_with_ellipsis(s, MAX_BASE_TITLE_CHARS);
        }
    }
    if let Some(line) = first_line {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            let stripped = strip_heading_prefix(trimmed);
            return clip_with_ellipsis(stripped, MAX_BASE_TITLE_CHARS);
        }
    }
    "Untitled".to_string()
}

fn clip_with_ellipsis(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    if max_chars == 0 {
        return String::new();
    }
    let mut s: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    s.push('…');
    s
}

/// Phase B15 — strip the leading `#` run + a single separator space
/// from a markdown heading. Lines that aren't headings pass through
/// unchanged. Lines that are *only* `#` characters (no body) also
/// pass through so they remain visible in the tab title.
fn strip_heading_prefix(line: &str) -> &str {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if hashes == 0 {
        return line;
    }
    let after_hashes = &line[hashes..];
    let after = after_hashes.trim_start_matches(' ');
    if after.is_empty() {
        return line;
    }
    after
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_buffer::BufferId;

    #[test]
    fn label_resolution_precedence() {
        let mut tab = Tab::new(BufferId::new(), 7);
        tab.label_override = Some("explicit".into());
        assert_eq!(resolve_label(&tab, Some("first line")), "explicit");
        tab.label_override = None;
        assert_eq!(resolve_label(&tab, Some("first line")), "first line");
        assert_eq!(resolve_label(&tab, Some("   ")), "Untitled");
        assert_eq!(resolve_label(&tab, None), "Untitled");
    }

    #[test]
    fn label_strips_markdown_heading_prefix() {
        let tab = Tab::new(BufferId::new(), 7);
        assert_eq!(resolve_label(&tab, Some("# Heading")), "Heading");
        assert_eq!(resolve_label(&tab, Some("## Deeper")), "Deeper");
        assert_eq!(resolve_label(&tab, Some("###    Padded")), "Padded");
    }

    #[test]
    fn label_keeps_hash_only_lines_visible() {
        let tab = Tab::new(BufferId::new(), 7);
        assert_eq!(resolve_label(&tab, Some("###")), "###");
    }

    #[test]
    fn label_does_not_strip_intra_word_hashes() {
        let tab = Tab::new(BufferId::new(), 7);
        assert_eq!(resolve_label(&tab, Some("hash#tag")), "hash#tag");
    }

    #[test]
    fn pinned_tab_label_prefixes_pin_dot() {
        let mut tab = Tab::new(BufferId::new(), 7);
        tab.label_override = Some("Notes".into());
        assert_eq!(resolve_label(&tab, None), "Notes");
        tab.pinned = true;
        assert_eq!(resolve_label(&tab, None), "\u{25CF} Notes");
    }

    #[test]
    fn label_caps_long_first_line_at_base_limit() {
        // Item 8 — the base title is capped at the generous
        // MAX_BASE_TITLE_CHARS (not the old 20), with ellipsis fitting to
        // the painted slot deferred to the renderer.
        let tab = Tab::new(BufferId::new(), 0);
        let long = "a".repeat(200);
        let s = resolve_label(&tab, Some(&long));
        assert!(s.ends_with('…'));
        assert_eq!(s.chars().count(), MAX_BASE_TITLE_CHARS);
    }

    #[test]
    fn label_under_cap_is_carried_verbatim() {
        // A 40-char first line is well under the 60-char cap, so it is
        // carried whole — the renderer decides how much fits.
        let tab = Tab::new(BufferId::new(), 0);
        let medium = "b".repeat(40);
        let s = resolve_label(&tab, Some(&medium));
        assert_eq!(s, medium);
        assert!(!s.ends_with('…'));
    }
}
