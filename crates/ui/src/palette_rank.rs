//! Command-palette-specific ranking.
//!
//! The shared fuzzy scorer stays small and generic; this module layers
//! command labels, aliases, separator normalization, and empty-query
//! defaults on top for the command palette only.

use continuity_search::{score, FuzzyMatch};

use crate::palette::PaletteEntry;

const DEFAULT_ORDER: &[&str] = &[
    "file.open",
    "file.open_folder",
    "file.save",
    "editor.find",
    "editor.replace",
    "editor.goto_line",
    "editor.goto_heading",
    "view.pick_theme",
    "settings.open",
    "view.toggle_wrap",
    "view.toggle_file_tree",
    "view.toggle_minimap",
    "view.toggle_outline",
    "help.tutorial",
];

/// Score one palette entry against the user's query.
pub(crate) fn score_entry(query: &str, entry: &PaletteEntry) -> Option<FuzzyMatch> {
    let normalized_query = normalize_for_match(query);
    if normalized_query.is_empty() {
        return Some(FuzzyMatch {
            score: default_priority(&entry.command),
            matched_indices: Vec::new(),
        });
    }
    let compact_query = compact(&normalized_query);
    let query_tokens = tokenize(&normalized_query);
    let mut best: Option<FuzzyMatch> = None;
    for field in fields_for(entry) {
        let normalized_field = normalize_for_match(&field);
        let compact_field = compact(&normalized_field);
        update_best(
            &mut best,
            score_field(
                &normalized_query,
                &compact_query,
                &query_tokens,
                &normalized_field,
            ),
        );
        update_best(
            &mut best,
            score_compact_field(&compact_query, &compact_field),
        );
    }
    best
}

fn update_best(best: &mut Option<FuzzyMatch>, candidate: Option<FuzzyMatch>) {
    let Some(candidate) = candidate else {
        return;
    };
    if best
        .as_ref()
        .is_none_or(|current| candidate.score > current.score)
    {
        *best = Some(candidate);
    }
}

fn score_field(
    normalized_query: &str,
    compact_query: &str,
    query_tokens: &[String],
    normalized_field: &str,
) -> Option<FuzzyMatch> {
    let mut fuzzy = score(normalized_query, normalized_field)?;
    fuzzy.score += field_boost(
        normalized_query,
        compact_query,
        query_tokens,
        normalized_field,
    );
    Some(fuzzy)
}

fn score_compact_field(compact_query: &str, compact_field: &str) -> Option<FuzzyMatch> {
    if compact_query.is_empty() {
        return None;
    }
    let mut fuzzy = score(compact_query, compact_field)?;
    fuzzy.score -= 8;
    if compact_field.contains(compact_query) {
        fuzzy.score += 90;
    }
    Some(fuzzy)
}

fn field_boost(
    normalized_query: &str,
    compact_query: &str,
    query_tokens: &[String],
    normalized_field: &str,
) -> i32 {
    let field_tokens = tokenize(normalized_field);
    let mut boost = 0;
    if normalized_field == normalized_query {
        boost += 420;
    } else if normalized_field.starts_with(normalized_query) {
        boost += 260;
    } else if contains_word_phrase(normalized_field, normalized_query) {
        boost += 220;
    }
    if compact(normalized_field) == compact_query {
        boost += 220;
    }
    if has_ordered_token_prefix_match(query_tokens, &field_tokens) {
        boost += 180;
    }
    for query_token in query_tokens {
        if field_tokens
            .iter()
            .any(|field_token| field_token == query_token)
        {
            boost += 80;
        } else if field_tokens
            .iter()
            .any(|field_token| field_token.starts_with(query_token))
        {
            boost += 64;
        }
    }
    boost
}

fn contains_word_phrase(field: &str, query: &str) -> bool {
    field == query
        || field.starts_with(&format!("{query} "))
        || field.ends_with(&format!(" {query}"))
        || field.contains(&format!(" {query} "))
}

fn has_ordered_token_prefix_match(query_tokens: &[String], field_tokens: &[String]) -> bool {
    if query_tokens.is_empty() {
        return false;
    }
    let mut field_idx = 0;
    for query_token in query_tokens {
        let Some(next_idx) = field_tokens
            .iter()
            .enumerate()
            .skip(field_idx)
            .find_map(|(idx, field_token)| field_token.starts_with(query_token).then_some(idx))
        else {
            return false;
        };
        field_idx = next_idx + 1;
    }
    true
}

fn default_priority(command: &str) -> i32 {
    DEFAULT_ORDER
        .iter()
        .position(|candidate| *candidate == command)
        .map(|idx| 10_000 - i32::try_from(idx).unwrap_or(0) * 100)
        .unwrap_or(0)
}

fn fields_for(entry: &PaletteEntry) -> Vec<String> {
    let mut fields = vec![
        label_from_command(&entry.command),
        normalize_command_id(&entry.command),
        entry.command.clone(),
    ];
    if let Some(description) = entry.description.as_ref() {
        fields.push(description.clone());
    }
    fields.extend(
        aliases_for(&entry.command)
            .iter()
            .map(|alias| (*alias).into()),
    );
    fields
}

fn aliases_for(command: &str) -> &'static [&'static str] {
    match command {
        "editor.find" => &["search", "find text", "find in buffer"],
        "editor.replace" => &["replace text", "find replace", "search replace"],
        "editor.goto_line" => &["go to line", "jump line", "line number"],
        "editor.goto_heading" => &["go to heading", "jump heading", "outline heading"],
        "file.open" => &["open file", "load file"],
        "file.open_folder" => &["open folder", "folder", "workspace", "file tree root"],
        "file.save" => &["save file", "write file", "export"],
        "file.save_as" => &["save as", "export as"],
        "help.tutorial" => &["tutorial", "help", "guide", "intro"],
        "palette.show" => &["command palette", "run command"],
        "settings.open" => &["settings", "preferences", "config", "configuration"],
        "view.pick_font" | "view.set_font_family" => {
            &["pick font", "choose font", "font picker", "typeface"]
        }
        "view.pick_theme" => &[
            "pick theme",
            "choose theme",
            "select theme",
            "theme picker",
            "appearance",
            "colors",
        ],
        "view.cycle_theme" => &["cycle theme", "next theme", "switch theme"],
        "view.toggle_file_tree" => &["file tree", "sidebar", "folder tree", "explorer"],
        "view.toggle_minimap" => &[
            "minimap",
            "mini map",
            "map",
            "overview",
            "document thumbnail",
        ],
        "view.toggle_outline" => &["outline", "table of contents", "toc", "headings"],
        "view.toggle_wrap" => &["wrap", "word wrap", "soft wrap", "line wrap"],
        "view.toggle_whitespace" => &["whitespace", "show spaces", "show tabs"],
        "view.toggle_line_numbers" => &["line numbers", "gutter numbers"],
        "view.toggle_relative_line_numbers" => &[
            "relative line numbers",
            "relative gutter numbers",
            "vim line numbers",
        ],
        "view.toggle_all_line_numbers" => &[
            "always show line numbers",
            "show all line numbers",
            "all gutter numbers",
        ],
        "view.toggle_focus_mode" => &["focus mode", "zen"],
        "view.toggle_distraction_free" => &["distraction free", "full screen", "focus"],
        _ => &[],
    }
}

fn label_from_command(command: &str) -> String {
    let tail = command.rsplit_once('.').map_or(command, |(_, tail)| tail);
    normalize_command_id(tail)
}

fn normalize_command_id(command: &str) -> String {
    normalize_for_match(command)
}

fn normalize_for_match(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut previous_was_space = true;
    for ch in input.chars() {
        if ch.is_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            previous_was_space = false;
        } else if !previous_was_space {
            out.push(' ');
            previous_was_space = true;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

fn compact(input: &str) -> String {
    input.chars().filter(|ch| ch.is_alphanumeric()).collect()
}

fn tokenize(input: &str) -> Vec<String> {
    input
        .split_whitespace()
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(command: &str) -> PaletteEntry {
        PaletteEntry {
            command: command.into(),
            keybinding: None,
            description: None,
            applicable: true,
        }
    }

    #[test]
    fn empty_query_uses_curated_default_priority() {
        let file_open = score_entry("", &entry("file.open")).unwrap();
        let alpha = score_entry("", &entry("editor.alpha")).unwrap();
        assert!(file_open.score > alpha.score);
    }

    #[test]
    fn spaces_match_underscore_command_names() {
        assert!(score_entry("pick theme", &entry("view.pick_theme")).is_some());
    }

    #[test]
    fn compact_query_matches_spaced_label() {
        assert!(score_entry("picktheme", &entry("view.pick_theme")).is_some());
    }

    #[test]
    fn token_prefix_query_scores_minimap() {
        let minimap = score_entry("mini", &entry("view.toggle_minimap")).unwrap();
        let line_numbers = score_entry("mini", &entry("view.toggle_line_numbers"));
        assert!(line_numbers.is_none_or(|score| minimap.score > score.score));
    }

    #[test]
    fn aliases_find_user_terms() {
        assert!(score_entry("preferences", &entry("settings.open")).is_some());
        assert!(score_entry("toc", &entry("view.toggle_outline")).is_some());
    }
}
