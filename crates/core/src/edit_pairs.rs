//! Phase-16.5 auto-pair planner.
//!
//! Sister module to [`crate::edit_inline`] etc. Builds
//! [`crate::selection_edit::SelectionEditPlan`]s for the two new
//! `SelectionEdit` variants the auto-pair feature added:
//!
//! * [`crate::SelectionEdit::InsertPair`] — inserts `open` immediately
//!   followed by `close` at every caret (or surrounds non-empty
//!   selections with the pair). Always one undo group.
//! * [`crate::SelectionEdit::DeletePair`] — backspace-aware delete that
//!   removes both halves when the caret sits between an empty pair, and
//!   returns `Ok(None)` otherwise so the caller can fall through to a
//!   plain [`crate::SelectionEdit::DeleteBack`].
//!
//! ## Configuration
//!
//! [`AutoPairConfig`] enumerates which characters should auto-pair.
//! The dispatch decision *which* `SelectionEdit` to enqueue lives in
//! the command/UI layer (so the planner stays free of any settings
//! plumbing) — this module just turns "you want a pair here" into a
//! plan. `Default` matches spec §12 prose-friendliness: `()`, `[]`,
//! `{}`, `""`, `''`, and `` ` `` are paired by default; `*` and `_`
//! are not.

use continuity_buffer::Buffer;
use continuity_text::{Position, Selection};
use ropey::Rope;

use crate::edit_planning::{advance_position, finalize_specs, EditSpec};
use crate::selection_edit::SelectionEditPlan;
use crate::Error;

/// Per-character toggle bag for the auto-pair feature. Mirrored on
/// [`continuity_config::Settings`] (see `[editor].auto_pair_*`).
#[derive(Copy, Clone, Debug)]
pub struct AutoPairConfig {
    /// `(` → `()`.
    pub paren: bool,
    /// `[` → `[]`.
    pub bracket: bool,
    /// `{` → `{}`.
    pub brace: bool,
    /// `"` → `""`.
    pub dquote: bool,
    /// `'` → `''`.
    pub squote: bool,
    /// `` ` `` → `` `` ``.
    pub backtick: bool,
    /// `*` → `**`. Off by default — hurts more than helps in prose.
    pub asterisk: bool,
    /// `_` → `__`. Off by default — same rationale.
    pub underscore: bool,
}

impl Default for AutoPairConfig {
    fn default() -> Self {
        Self {
            paren: true,
            bracket: true,
            brace: true,
            dquote: true,
            squote: true,
            backtick: true,
            asterisk: false,
            underscore: false,
        }
    }
}

impl AutoPairConfig {
    /// If `c` is one of the configured open characters, return its
    /// `(open, close)` pair. Otherwise `None`.
    #[must_use]
    pub fn pair_for(&self, c: char) -> Option<(char, char)> {
        match c {
            '(' if self.paren => Some(('(', ')')),
            '[' if self.bracket => Some(('[', ']')),
            '{' if self.brace => Some(('{', '}')),
            '"' if self.dquote => Some(('"', '"')),
            '\'' if self.squote => Some(('\'', '\'')),
            '`' if self.backtick => Some(('`', '`')),
            '*' if self.asterisk => Some(('*', '*')),
            '_' if self.underscore => Some(('_', '_')),
            _ => None,
        }
    }
}

/// Plan one auto-pair insertion per active selection. Empty (caret)
/// selections insert `open` immediately followed by `close` and leave
/// the caret between them. Non-empty selections wrap the range with the
/// pair and leave the caret just after the trailing `close`.
///
/// # Errors
///
/// Returns the buffer-layer error if any selection position is outside
/// the rope.
pub(crate) fn plan_insert_pair(
    buffer: &Buffer,
    open: &str,
    close: &str,
) -> Result<Option<SelectionEditPlan>, Error> {
    let selections_before = buffer.selections().to_vec();
    let rope = buffer.rope();
    let mut specs: Vec<EditSpec> = Vec::with_capacity(selections_before.len() * 2);
    let mut selections_after: Vec<Selection> = Vec::with_capacity(selections_before.len());
    for selection in &selections_before {
        let ordered = selection.ordered_range();
        let start_byte = ordered.start.to_byte_offset(rope)?;
        let end_byte = ordered.end.to_byte_offset(rope)?;
        if start_byte == end_byte {
            // Caret: insert "open close" at the caret, leave the caret
            // sitting between them. The post-edit caret position is
            // `start_position` advanced by `open` (no newlines in pair
            // chars in practice, but `advance_position` handles them
            // correctly anyway).
            let combined = format!("{open}{close}");
            specs.push(EditSpec::insert(rope, start_byte, combined)?);
            let head = advance_position(ordered.start, open);
            selections_after.push(Selection::caret_at(head));
        } else {
            // Non-empty selection: wrap as one replace spec covering
            // [start, end) with `open + content + close`. Using a
            // single replace (rather than two paired inserts) keeps the
            // post-edit caret math local — it is just `start_position`
            // advanced by the entire replacement string.
            let start_char = rope.byte_to_char(start_byte);
            let end_char = rope.byte_to_char(end_byte);
            let content: String = rope.slice(start_char..end_char).into();
            let wrapped = format!("{open}{content}{close}");
            specs.push(EditSpec::replace(
                rope,
                start_byte,
                end_byte,
                wrapped.clone(),
            )?);
            let head = advance_position(ordered.start, &wrapped);
            selections_after.push(Selection::caret_at(head));
        }
    }
    Ok(finalize_specs(specs, selections_before, selections_after))
}

/// Plan a delete-pair backspace: at each caret, if the byte immediately
/// before the caret is `open` and the byte immediately after is
/// `close`, delete both. Returns `Ok(None)` when no caret matches —
/// the caller should then fall through to the plain
/// [`crate::SelectionEdit::DeleteBack`] planner.
///
/// # Errors
///
/// Returns the buffer-layer error if any selection position is outside
/// the rope.
pub(crate) fn plan_delete_pair(
    buffer: &Buffer,
    open: char,
    close: char,
) -> Result<Option<SelectionEditPlan>, Error> {
    let selections_before = buffer.selections().to_vec();
    let rope = buffer.rope();
    let mut specs: Vec<EditSpec> = Vec::with_capacity(selections_before.len());
    let mut selections_after: Vec<Selection> = Vec::with_capacity(selections_before.len());
    for selection in &selections_before {
        let ordered = selection.ordered_range();
        let start_byte = ordered.start.to_byte_offset(rope)?;
        let end_byte = ordered.end.to_byte_offset(rope)?;
        if start_byte != end_byte {
            // A non-empty selection isn't an auto-pair backspace — bail
            // out so the caller falls through to plain DeleteBack.
            return Ok(None);
        }
        let Some(prev) = char_before(rope, start_byte) else {
            return Ok(None);
        };
        let Some(next) = char_at(rope, start_byte) else {
            return Ok(None);
        };
        if prev != open || next != close {
            return Ok(None);
        }
        let prev_len = prev.len_utf8();
        let next_len = next.len_utf8();
        let delete_start = start_byte - prev_len;
        let delete_end = start_byte + next_len;
        specs.push(EditSpec::delete(rope, delete_start, delete_end)?);
        selections_after.push(Selection::caret_at(position_for_byte(rope, delete_start)?));
    }
    Ok(finalize_specs(specs, selections_before, selections_after))
}

fn position_for_byte(rope: &Rope, byte: usize) -> Result<Position, Error> {
    Ok(Position::from_byte_offset(rope, byte)?)
}

fn char_before(rope: &Rope, byte: usize) -> Option<char> {
    if byte == 0 {
        return None;
    }
    // Rewind one char by walking back up to 4 bytes (longest UTF-8).
    let lo = byte.saturating_sub(4);
    let mut start = lo;
    while start < byte {
        if let Ok(pos) = Position::from_byte_offset(rope, start) {
            let _ = pos;
            // Round to the nearest grapheme/codepoint boundary by
            // re-decoding the slice [start, byte).
            let slice = rope.slice(rope.byte_to_char(start)..rope.byte_to_char(byte));
            let s: String = slice.chars().collect();
            return s.chars().next_back();
        }
        start += 1;
    }
    None
}

fn char_at(rope: &Rope, byte: usize) -> Option<char> {
    if byte >= rope.len_bytes() {
        return None;
    }
    let char_idx = rope.byte_to_char(byte);
    rope.chars_at(char_idx).next()
}

#[cfg(test)]
mod tests {
    use continuity_buffer::Buffer;
    use continuity_text::{Position, Selection};

    use super::*;
    use crate::selection_edit::apply_plan;

    #[test]
    fn defaults_pair_brackets_not_emphasis() {
        let cfg = AutoPairConfig::default();
        assert_eq!(cfg.pair_for('('), Some(('(', ')')));
        assert_eq!(cfg.pair_for('['), Some(('[', ']')));
        assert_eq!(cfg.pair_for('{'), Some(('{', '}')));
        assert_eq!(cfg.pair_for('"'), Some(('"', '"')));
        assert_eq!(cfg.pair_for('*'), None);
        assert_eq!(cfg.pair_for('_'), None);
    }

    #[test]
    fn insert_pair_at_caret_lands_between() {
        let mut buffer = Buffer::from_text("ab");
        buffer.set_selections(vec![Selection::caret_at(Position::new(0, 1))]);
        let plan = plan_insert_pair(&buffer, "(", ")")
            .expect("plan ok")
            .expect("plan some");
        assert_eq!(plan.ops.len(), 1);
        apply_plan(&mut buffer, &plan).expect("apply ok");
        assert_eq!(buffer.rope().to_string(), "a()b");
        let head = buffer.selections()[0].head;
        assert_eq!((head.line, head.byte_in_line), (0, 2));
    }

    #[test]
    fn insert_pair_wraps_non_empty_selection() {
        let mut buffer = Buffer::from_text("hello");
        buffer.set_selections(vec![Selection::new(
            Position::new(0, 0),
            Position::new(0, 5),
            continuity_text::SelectionKind::Caret,
        )]);
        let plan = plan_insert_pair(&buffer, "[", "]")
            .expect("plan ok")
            .expect("plan some");
        apply_plan(&mut buffer, &plan).expect("apply ok");
        assert_eq!(buffer.rope().to_string(), "[hello]");
    }

    #[test]
    fn delete_pair_between_empty_pair_succeeds() {
        let mut buffer = Buffer::from_text("()");
        buffer.set_selections(vec![Selection::caret_at(Position::new(0, 1))]);
        let plan = plan_delete_pair(&buffer, '(', ')')
            .expect("plan ok")
            .expect("plan some");
        apply_plan(&mut buffer, &plan).expect("apply ok");
        assert_eq!(buffer.rope().to_string(), "");
    }

    #[test]
    fn delete_pair_when_not_adjacent_returns_none() {
        // Caret between `(` and `a` — `next` is `a`, not `)`, so no pair.
        let mut buf = Buffer::from_text("(a)");
        buf.set_selections(vec![Selection::caret_at(Position::new(0, 1))]);
        let plan = plan_delete_pair(&buf, '(', ')').expect("plan ok");
        assert!(plan.is_none());
    }
}
