//! Line-start bullet toggling — the `Ctrl+R` quick-toggle and the
//! `Ctrl+Shift+R` toggle-with-continuation-indent variant.
//!
//! Both scan every covered line first, then pick one action globally so a
//! multi-line selection converges deterministically. The detection of an
//! existing list prefix (dash-style `- ` / `* ` / `+ ` **and** ordered
//! `N. ` / `N) `) reuses
//! [`crate::edit_markdown::split_leading_list_marker`] so an ordered line is
//! recognised as already carrying a list prefix rather than getting a stray
//! `- ` prepended in front of `1. `.

use std::collections::HashMap;

use continuity_buffer::Buffer;
use continuity_text::{Position, Selection};

use crate::edit_markdown::split_leading_list_marker;
use crate::edit_planning::{finalize_specs, line_content_end, EditSpec};
use crate::selection_edit::SelectionEditPlan;
use crate::Error;

use super::{leading_whitespace, lines_covered};

const ADD_PREFIX: &str = "- ";

/// Per-covered-line snapshot driving the global decision and the post-edit
/// caret-column shift.
struct LineInfo {
    line: usize,
    leading_len: usize,
    line_start: usize,
    line_end: usize,
    line_text: String,
    /// Byte length of the existing list-marker prefix (after leading
    /// whitespace), or `None` when the line carries no list marker.
    marker_len: Option<usize>,
    /// `true` when the existing marker is a dash-style bullet (`- ` / `* `
    /// / `+ `) rather than an ordered `N. ` / `N) ` marker.
    is_dash_bullet: bool,
    /// Line body with the existing marker stripped off (only meaningful
    /// when `marker_len.is_some()`).
    body_after_marker: String,
}

/// Per-line caret-column delta applied to anchor/head positions.
enum Delta {
    /// `+n` for columns at or past `leading_len`.
    Add(i32),
    /// `-prefix_len` for columns at or past `leading_len + prefix_len`,
    /// clamped to `leading_len` inside the marker run.
    Remove { prefix_len: usize },
    /// No edit on this line.
    None,
}

/// Toggle a `- ` bullet marker at the start of every covered line (after any
/// leading whitespace). Bound to `Ctrl+R`.
///
/// Multi-line behaviour (matches VS Code / Obsidian): scan every covered
/// line first, then pick one action globally —
/// - **All lines are dash-style bullets** → strip the marker from each
///   (→ plain text).
/// - **Otherwise** → "make bullets": ordered (`N. ` / `N) `) lines have
///   their marker **replaced** by `- `, unmarked lines gain `- `, and
///   existing dash bullets stay untouched. An ordered line therefore goes
///   ordered → bullet → plain across two presses.
///
/// Caret column shifts so the cursor visually stays on the same content
/// character: `+2` on add, `2 - ordered_marker_len` on an ordered→bullet
/// conversion, `-prefix_len` on remove (clamped to the leading-whitespace
/// column). This holds for both endpoints of a multi-line range selection.
pub(crate) fn plan_toggle_bullet_at_line_start(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let Some(infos) = collect_covered_line_infos(buffer) else {
        return Ok(None);
    };

    // Strip only when every covered line is already a dash-style bullet;
    // ordered lines count as "having a prefix" for nothing here — they are
    // converted to bullets first. This yields ordered → bullet → plain.
    let all_dash_bulleted = infos.iter().all(|i| i.is_dash_bullet);

    let mut specs = Vec::new();
    let mut deltas: HashMap<usize, (usize, Delta)> = HashMap::new();

    for info in &infos {
        let indent = &info.line_text[..info.leading_len];
        if all_dash_bulleted {
            // Strip the dash marker.
            let prefix_len = info.marker_len.expect("all_dash_bulleted ⇒ marker present");
            let new_line = format!("{indent}{}", info.body_after_marker);
            specs.push(EditSpec::replace(
                rope,
                info.line_start,
                info.line_end,
                new_line,
            )?);
            deltas.insert(info.line, (info.leading_len, Delta::Remove { prefix_len }));
        } else if info.is_dash_bullet {
            // Already a dash bullet, but we're in make-bullets mode: leave
            // it untouched so a mixed selection converges to all-dash.
            deltas.insert(info.line, (info.leading_len, Delta::None));
        } else if let Some(ordered_len) = info.marker_len {
            // Ordered marker → replace with `- `.
            let new_line = format!("{indent}{ADD_PREFIX}{}", info.body_after_marker);
            specs.push(EditSpec::replace(
                rope,
                info.line_start,
                info.line_end,
                new_line,
            )?);
            let delta = ADD_PREFIX.len() as i32 - ordered_len as i32;
            if delta >= 0 {
                deltas.insert(info.line, (info.leading_len, Delta::Add(delta)));
            } else {
                deltas.insert(
                    info.line,
                    (
                        info.leading_len,
                        Delta::Remove {
                            prefix_len: (-delta) as usize,
                        },
                    ),
                );
            }
        } else {
            // Plain line → add `- `.
            let body = &info.line_text[info.leading_len..];
            let new_line = format!("{indent}{ADD_PREFIX}{body}");
            specs.push(EditSpec::replace(
                rope,
                info.line_start,
                info.line_end,
                new_line,
            )?);
            deltas.insert(
                info.line,
                (info.leading_len, Delta::Add(ADD_PREFIX.len() as i32)),
            );
        }
    }

    let selections_after = shift_selections(&selections_before, &deltas);
    Ok(finalize_specs(specs, selections_before, selections_after))
}

/// Toggle a `- ` bullet on the covered lines and, for a multi-line
/// selection, indent every line after the first by one `indent_unit` on the
/// add path (and remove that indent on the strip path). Bound to
/// `Ctrl+Shift+R`. A single-line selection behaves exactly like
/// [`plan_toggle_bullet_at_line_start`].
pub(crate) fn plan_toggle_bullet_with_continuation_indent(
    buffer: &Buffer,
    indent_unit: &str,
) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let Some(infos) = collect_covered_line_infos(buffer) else {
        return Ok(None);
    };

    // Single covered line → identical to the plain Ctrl+R toggle.
    if infos.len() <= 1 {
        return plan_toggle_bullet_at_line_start(buffer);
    }

    let all_dash_bulleted = infos.iter().all(|i| i.is_dash_bullet);
    let first_line = infos.first().expect("non-empty by guard").line;

    let mut specs = Vec::new();
    let mut deltas: HashMap<usize, (usize, Delta)> = HashMap::new();

    for info in &infos {
        let indent = &info.line_text[..info.leading_len];
        // Continuation lines (every covered line after the first) carry the
        // extra indent on the add path / lose it on the strip path.
        let is_continuation = info.line != first_line;
        if all_dash_bulleted {
            // Strip the dash marker, and drop one indent unit from each
            // continuation line's leading whitespace if present.
            let prefix_len = info.marker_len.expect("all_dash_bulleted ⇒ marker present");
            let (kept_indent, dropped_indent_len) =
                if is_continuation && indent.starts_with(indent_unit) {
                    (&indent[indent_unit.len()..], indent_unit.len())
                } else {
                    (indent, 0)
                };
            let new_line = format!("{kept_indent}{}", info.body_after_marker);
            specs.push(EditSpec::replace(
                rope,
                info.line_start,
                info.line_end,
                new_line,
            )?);
            deltas.insert(
                info.line,
                (
                    info.leading_len,
                    Delta::Remove {
                        prefix_len: prefix_len + dropped_indent_len,
                    },
                ),
            );
        } else {
            // Make-bullets mode. Continuation lines also gain one indent
            // unit ahead of the (possibly converted) marker.
            let extra_indent = if is_continuation { indent_unit } else { "" };
            let body_no_marker = match info.marker_len {
                Some(_) => info.body_after_marker.clone(),
                None => info.line_text[info.leading_len..].to_string(),
            };
            let new_line = format!("{extra_indent}{indent}{ADD_PREFIX}{body_no_marker}");
            specs.push(EditSpec::replace(
                rope,
                info.line_start,
                info.line_end,
                new_line,
            )?);
            // Column delta: `+indent_unit + ADD_PREFIX - old_marker_len`.
            let old_marker = info.marker_len.unwrap_or(0) as i32;
            let added = extra_indent.len() as i32 + ADD_PREFIX.len() as i32;
            let delta = added - old_marker;
            if delta >= 0 {
                deltas.insert(info.line, (info.leading_len, Delta::Add(delta)));
            } else {
                deltas.insert(
                    info.line,
                    (
                        info.leading_len,
                        Delta::Remove {
                            prefix_len: (-delta) as usize,
                        },
                    ),
                );
            }
        }
    }

    let selections_after = shift_selections(&selections_before, &deltas);
    Ok(finalize_specs(specs, selections_before, selections_after))
}

/// Snapshot every covered line for the toggle decision. Returns `None` when
/// no lines are covered (after the multi-line blank-line filter).
fn collect_covered_line_infos(buffer: &Buffer) -> Option<Vec<LineInfo>> {
    let rope = buffer.rope();
    let covered = lines_covered(buffer);
    if covered.is_empty() {
        return None;
    }
    // Multi-line toggles skip blank / whitespace-only lines: bulleting a
    // selection that spans paragraph gaps must not mint markers on the
    // gaps. A caret on a single blank line still toggles, so a writer can
    // start a list from an empty line.
    let covered: Vec<usize> = if covered.len() > 1 {
        covered
            .into_iter()
            .filter(|&line| {
                let start = rope.line_to_byte(line);
                let end = line_content_end(rope, line);
                end - start > leading_whitespace(rope, line).len()
            })
            .collect()
    } else {
        covered
    };
    if covered.is_empty() {
        return None;
    }

    let mut infos = Vec::with_capacity(covered.len());
    for &line in &covered {
        let line_start = rope.line_to_byte(line);
        let line_end = line_content_end(rope, line);
        let line_text = rope.byte_slice(line_start..line_end).to_string();
        let leading_len = leading_whitespace(rope, line).len();
        let body = &line_text[leading_len..];
        let (marker, rest) = split_leading_list_marker(body);
        let (marker_len, is_dash_bullet, body_after_marker) = if marker.is_empty() {
            (None, false, String::new())
        } else {
            let is_dash = matches!(marker, "- " | "* " | "+ ");
            (Some(marker.len()), is_dash, rest.to_string())
        };
        infos.push(LineInfo {
            line,
            leading_len,
            line_start,
            line_end,
            line_text,
            marker_len,
            is_dash_bullet,
            body_after_marker,
        });
    }
    Some(infos)
}

fn shift_selections(
    selections_before: &[Selection],
    deltas: &HashMap<usize, (usize, Delta)>,
) -> Vec<Selection> {
    let shift = |p: Position| -> Position {
        let Some((leading_len, delta)) = deltas.get(&(p.line as usize)) else {
            return p;
        };
        let col = p.byte_in_line as usize;
        let new_col = match delta {
            Delta::None => col,
            Delta::Add(n) => {
                if col >= *leading_len {
                    (col as i32 + *n).max(0) as usize
                } else {
                    col
                }
            }
            Delta::Remove { prefix_len } => {
                if col >= leading_len + prefix_len {
                    col - prefix_len
                } else {
                    col.min(*leading_len)
                }
            }
        };
        Position::new(p.line, new_col as u32)
    };
    selections_before
        .iter()
        .map(|sel| Selection {
            anchor: shift(sel.anchor),
            head: shift(sel.head),
            kind: sel.kind,
        })
        .collect()
}
