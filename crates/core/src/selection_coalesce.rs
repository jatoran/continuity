//! Multi-cursor coalescing (Phase B1).
//!
//! Tiny helper extracted from [`crate::selection_edit`] to keep that
//! file under the 600-line cap. The rule: at the end of every
//! selection-aware edit and every motion command, drop selections that
//! collide on the full `(anchor, head, kind)` tuple — silent duplicates
//! cause double-inserts on the next typed character.

use continuity_text::Selection;

/// Drop selections with identical `(anchor, head, kind)` tuples while
/// preserving order. AHashSet-backed, O(n).
pub(crate) fn coalesce_selections(selections: &mut Vec<Selection>) {
    if selections.len() <= 1 {
        return;
    }
    let mut seen = ahash::AHashSet::with_capacity(selections.len());
    selections.retain(|sel| seen.insert(*sel));
}

#[cfg(test)]
mod tests {
    use super::coalesce_selections;
    use continuity_text::{Position, Selection, SelectionKind};

    #[test]
    fn drops_identical_carets() {
        let mut sels = vec![
            Selection::caret_at(Position::new(0, 2)),
            Selection::caret_at(Position::new(0, 5)),
            Selection::caret_at(Position::new(0, 2)),
        ];
        coalesce_selections(&mut sels);
        assert_eq!(
            sels,
            vec![
                Selection::caret_at(Position::new(0, 2)),
                Selection::caret_at(Position::new(0, 5)),
            ]
        );
    }

    #[test]
    fn preserves_order() {
        let mut sels = vec![
            Selection::caret_at(Position::new(0, 9)),
            Selection::caret_at(Position::new(0, 1)),
            Selection::caret_at(Position::new(0, 9)),
            Selection::caret_at(Position::new(0, 5)),
        ];
        coalesce_selections(&mut sels);
        assert_eq!(
            sels,
            vec![
                Selection::caret_at(Position::new(0, 9)),
                Selection::caret_at(Position::new(0, 1)),
                Selection::caret_at(Position::new(0, 5)),
            ]
        );
    }

    #[test]
    fn distinguishes_kind() {
        let p = Position::new(0, 3);
        let mut sels = vec![
            Selection::caret_at(p),
            Selection::new(p, p, SelectionKind::LineWise),
        ];
        coalesce_selections(&mut sels);
        assert_eq!(sels.len(), 2);
    }

    #[test]
    fn distinguishes_anchor() {
        let head = Position::new(0, 4);
        let mut sels = vec![
            Selection::new(Position::new(0, 1), head, SelectionKind::Caret),
            Selection::new(Position::new(0, 2), head, SelectionKind::Caret),
        ];
        coalesce_selections(&mut sels);
        assert_eq!(sels.len(), 2);
    }

    #[test]
    fn noop_on_empty_or_single() {
        let mut empty: Vec<Selection> = Vec::new();
        coalesce_selections(&mut empty);
        assert!(empty.is_empty());
        let mut one = vec![Selection::caret_at(Position::new(0, 1))];
        coalesce_selections(&mut one);
        assert_eq!(one.len(), 1);
    }
}
