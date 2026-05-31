//! Phase-A palette-mode framework.
//!
//! Generalizes the command palette into a reusable transient picker with
//! three callbacks per mode: **preview** on hover, **commit** on Enter,
//! **revert** on Escape. The same machinery powers font picker, theme
//! picker, math eval, hold-modifier chord HUD, Ctrl+Tab overlay, slash
//! commands, and the timeline scrubber.
//!
//! This file defines the types only. Wiring into the existing palette
//! renderer happens in a follow-up; see `roadmap_v2.md` §E (palette +
//! theme + font) and §I1 (timeline scrubber) for consumers.
//!
//! Thread ownership: UI thread of one window. A [`PaletteSession`] is
//! owned by `crate::Window` and lives only between palette-open and
//! palette-close.

use std::fmt;

use crate::Error;

/// One row in a palette result list.
///
/// `label` is the primary text rendered for the row; `hint` is an optional
/// secondary line (currently a keybind chord, command description, font
/// preview sample, etc. — the [`PaletteMode`] picks the semantics).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteRow {
    /// Primary text.
    pub label: String,
    /// Optional secondary text (chord, description, preview sample).
    pub hint: Option<String>,
}

impl PaletteRow {
    /// Construct a row with only a label.
    #[must_use]
    pub fn label_only<S: Into<String>>(label: S) -> Self {
        Self {
            label: label.into(),
            hint: None,
        }
    }

    /// Construct a row with both a label and a hint.
    #[must_use]
    pub fn with_hint<L: Into<String>, H: Into<String>>(label: L, hint: H) -> Self {
        Self {
            label: label.into(),
            hint: Some(hint.into()),
        }
    }
}

/// One mode of the palette. Modes own their own filtering logic and the
/// three transient callbacks.
///
/// Implementations are expected to be stateless across rows beyond what
/// they need for `preview` rollback — the [`PaletteSession`] tracks the
/// currently highlighted row, not the mode itself.
pub trait PaletteMode {
    /// Return rows matching `query`. Called every time the filter line
    /// changes. The order returned is the order rendered.
    fn filter(&self, query: &str) -> Vec<PaletteRow>;

    /// Apply a preview state for `row`. Called when the user moves the
    /// highlight (arrow keys, click). May mutate the editor view without
    /// persisting — e.g. font picker swaps `view.font_family`, theme
    /// picker swaps the active theme. Errors are swallowed by the caller
    /// (preview must never break the editor).
    ///
    /// Default implementation is a no-op. Modes that have no preview
    /// (math eval, command palette) leave it default.
    fn preview(&mut self, _row: &PaletteRow) -> Result<(), Error> {
        Ok(())
    }

    /// Commit `row` as the final selection. Called on Enter.
    /// Implementations should make the choice durable: write through to
    /// settings (via the round-trip path), apply the chosen value, etc.
    fn commit(&mut self, row: &PaletteRow) -> Result<(), Error>;

    /// Cancel the session. Called on Esc *and* when the palette closes
    /// without commit. Implementations should undo any pending preview
    /// state so the editor returns to its pre-open snapshot. The
    /// [`PaletteSession`] guarantees this is called exactly once per
    /// session.
    ///
    /// Default implementation is a no-op for modes that have no preview.
    fn cancel(&mut self) {}
}

/// One palette session. Wraps a mode, tracks the filter line, the current
/// highlight, and ensures the cancel callback fires exactly once when the
/// session is dropped without commit.
pub struct PaletteSession<M: PaletteMode> {
    mode: M,
    query: String,
    rows: Vec<PaletteRow>,
    /// Index into `rows` of the highlighted row, or `None` when `rows`
    /// is empty.
    selected: Option<usize>,
    /// True once `commit` or `cancel` has been called. Subsequent drops
    /// won't double-fire.
    settled: bool,
}

impl<M: PaletteMode> PaletteSession<M> {
    /// Open a session with `initial_query` (often empty) and produce the
    /// first row set.
    #[must_use]
    pub fn open(mode: M, initial_query: String) -> Self {
        let mut session = Self {
            mode,
            query: String::new(),
            rows: Vec::new(),
            selected: None,
            settled: false,
        };
        session.query = initial_query;
        session.rows = session.mode.filter(&session.query);
        session.selected = (!session.rows.is_empty()).then_some(0);
        // Fire preview on the initial highlight so the editor reflects the
        // top result immediately.
        session.preview_current();
        session
    }

    /// Currently entered filter string.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Read-only view of the current row list.
    #[must_use]
    pub fn rows(&self) -> &[PaletteRow] {
        &self.rows
    }

    /// Currently highlighted row, if any.
    #[must_use]
    pub fn current(&self) -> Option<&PaletteRow> {
        self.selected.and_then(|i| self.rows.get(i))
    }

    /// Index of the highlighted row, if any.
    #[must_use]
    pub fn selected_index(&self) -> Option<usize> {
        self.selected
    }

    /// Replace the filter line. Refreshes rows + preserves selection by
    /// label when possible; otherwise resets to the first row. Fires the
    /// mode's preview callback for the new highlight.
    pub fn set_query<S: Into<String>>(&mut self, q: S) {
        self.query = q.into();
        let prev_label = self.current().map(|r| r.label.clone());
        self.rows = self.mode.filter(&self.query);
        self.selected = prev_label
            .and_then(|label| self.rows.iter().position(|r| r.label == label))
            .or_else(|| (!self.rows.is_empty()).then_some(0));
        self.preview_current();
    }

    /// Move highlight by `delta` (positive = down). Wraps. No-op when
    /// there are no rows. Fires preview for the new selection.
    pub fn step(&mut self, delta: i32) {
        if self.rows.is_empty() {
            return;
        }
        let len = self.rows.len() as i32;
        let cur = self.selected.unwrap_or(0) as i32;
        let mut next = (cur + delta) % len;
        if next < 0 {
            next += len;
        }
        self.selected = Some(next as usize);
        self.preview_current();
    }

    /// Commit the highlighted row. After this returns the session is
    /// settled — further `commit`/`cancel` calls are no-ops.
    ///
    /// # Errors
    ///
    /// Returns whatever the mode's `commit` callback returns. Caller is
    /// responsible for displaying a banner if needed.
    pub fn commit(&mut self) -> Result<(), Error> {
        if self.settled {
            return Ok(());
        }
        self.settled = true;
        let Some(row) = self.current().cloned() else {
            // Nothing to commit. Still treat as settled to suppress
            // a stray cancel-on-drop.
            return Ok(());
        };
        self.mode.commit(&row)
    }

    /// Cancel the session. Calls the mode's `cancel` callback exactly
    /// once. After this returns the session is settled.
    pub fn cancel(&mut self) {
        if self.settled {
            return;
        }
        self.settled = true;
        self.mode.cancel();
    }

    /// Whether `commit` or `cancel` has already been called.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.settled
    }

    fn preview_current(&mut self) {
        if let Some(row) = self.current().cloned() {
            // Preview errors are intentionally swallowed — a broken
            // preview must never wedge the palette.
            let _ = self.mode.preview(&row);
        }
    }
}

impl<M: PaletteMode> Drop for PaletteSession<M> {
    fn drop(&mut self) {
        if !self.settled {
            self.settled = true;
            self.mode.cancel();
        }
    }
}

impl<M: PaletteMode> fmt::Debug for PaletteSession<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaletteSession")
            .field("query", &self.query)
            .field("rows", &self.rows.len())
            .field("selected", &self.selected)
            .field("settled", &self.settled)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// Trace the mode's callback log so tests can assert on exact ordering.
    #[derive(Default, Clone)]
    struct Trace(Rc<RefCell<Vec<String>>>);

    impl Trace {
        fn push(&self, s: impl Into<String>) {
            self.0.borrow_mut().push(s.into());
        }
        fn log(&self) -> Vec<String> {
            self.0.borrow().clone()
        }
    }

    /// Test mode: filters a fixed list by substring; tracks preview /
    /// commit / cancel calls.
    struct StubMode {
        rows: Vec<String>,
        trace: Trace,
        commit_fails: bool,
    }

    impl PaletteMode for StubMode {
        fn filter(&self, query: &str) -> Vec<PaletteRow> {
            self.rows
                .iter()
                .filter(|r| r.contains(query))
                .cloned()
                .map(PaletteRow::label_only)
                .collect()
        }
        fn preview(&mut self, row: &PaletteRow) -> Result<(), Error> {
            self.trace.push(format!("preview:{}", row.label));
            Ok(())
        }
        fn commit(&mut self, row: &PaletteRow) -> Result<(), Error> {
            self.trace.push(format!("commit:{}", row.label));
            if self.commit_fails {
                Err(Error::Command(continuity_command::Error::Other(
                    "test commit failure".into(),
                )))
            } else {
                Ok(())
            }
        }
        fn cancel(&mut self) {
            self.trace.push("cancel");
        }
    }

    fn session(rows: Vec<&str>) -> (PaletteSession<StubMode>, Trace) {
        let trace = Trace::default();
        let mode = StubMode {
            rows: rows.into_iter().map(String::from).collect(),
            trace: trace.clone(),
            commit_fails: false,
        };
        (PaletteSession::open(mode, String::new()), trace)
    }

    #[test]
    fn open_fires_initial_preview_on_first_row() {
        let (sess, trace) = session(vec!["alpha", "beta", "gamma"]);
        assert_eq!(sess.selected_index(), Some(0));
        assert_eq!(sess.current().unwrap().label, "alpha");
        assert_eq!(trace.log(), vec!["preview:alpha"]);
    }

    #[test]
    fn open_with_empty_rows_has_no_selection() {
        let trace = Trace::default();
        let mode = StubMode {
            rows: vec![],
            trace: trace.clone(),
            commit_fails: false,
        };
        let sess = PaletteSession::open(mode, String::new());
        assert!(sess.current().is_none());
        assert_eq!(sess.selected_index(), None);
        assert_eq!(trace.log().len(), 0);
    }

    #[test]
    fn step_wraps_and_fires_preview() {
        let (mut sess, trace) = session(vec!["one", "two", "three"]);
        sess.step(1);
        sess.step(1);
        sess.step(1);
        // Wrap around.
        sess.step(1);
        assert_eq!(sess.current().unwrap().label, "two");
        assert_eq!(
            trace.log(),
            vec![
                "preview:one",
                "preview:two",
                "preview:three",
                "preview:one",
                "preview:two"
            ]
        );
    }

    #[test]
    fn step_backward_wraps_to_last() {
        let (mut sess, _) = session(vec!["a", "b", "c"]);
        sess.step(-1);
        assert_eq!(sess.current().unwrap().label, "c");
        sess.step(-1);
        assert_eq!(sess.current().unwrap().label, "b");
    }

    #[test]
    fn set_query_filters_and_preserves_selection_by_label() {
        let (mut sess, trace) = session(vec!["alpha", "alphabet", "beta"]);
        sess.step(1); // → "alphabet"
        sess.set_query("alpha");
        assert_eq!(sess.current().unwrap().label, "alphabet");
        // Then narrow to only "alphabet" — still on alphabet.
        sess.set_query("bet");
        assert_eq!(sess.current().unwrap().label, "alphabet");
        // Then a query with no overlap — falls back to first row.
        sess.set_query("zzz");
        assert!(sess.current().is_none());
        // Drop trace usage to ensure compilation.
        let _ = trace.log();
    }

    #[test]
    fn commit_fires_callback_once_then_settles() {
        let (mut sess, trace) = session(vec!["x", "y"]);
        sess.commit().unwrap();
        assert!(sess.is_settled());
        // Subsequent commits are no-ops.
        sess.commit().unwrap();
        assert_eq!(trace.log(), vec!["preview:x", "commit:x"]);
    }

    #[test]
    fn cancel_fires_callback_once_then_settles() {
        let (mut sess, trace) = session(vec!["x"]);
        sess.cancel();
        sess.cancel(); // no-op
        drop(sess);
        assert_eq!(trace.log(), vec!["preview:x", "cancel"]);
    }

    #[test]
    fn drop_without_commit_or_cancel_fires_cancel() {
        let (sess, trace) = session(vec!["x"]);
        drop(sess);
        assert_eq!(trace.log(), vec!["preview:x", "cancel"]);
    }

    #[test]
    fn commit_suppresses_drop_cancel() {
        let (mut sess, trace) = session(vec!["x"]);
        sess.commit().unwrap();
        drop(sess);
        // No trailing cancel.
        assert_eq!(trace.log(), vec!["preview:x", "commit:x"]);
    }

    #[test]
    fn commit_with_no_rows_is_noop_but_settles() {
        let trace = Trace::default();
        let mode = StubMode {
            rows: vec![],
            trace: trace.clone(),
            commit_fails: false,
        };
        let mut sess = PaletteSession::open(mode, String::new());
        sess.commit().unwrap();
        drop(sess);
        // No callbacks fired at all.
        assert!(trace.log().is_empty());
    }

    #[test]
    fn commit_failure_still_settles() {
        let trace = Trace::default();
        let mode = StubMode {
            rows: vec!["x".into()],
            trace: trace.clone(),
            commit_fails: true,
        };
        let mut sess = PaletteSession::open(mode, String::new());
        let err = sess.commit().unwrap_err();
        assert!(matches!(err, Error::Command(_)));
        // Still settled — drop won't fire cancel.
        drop(sess);
        assert_eq!(trace.log(), vec!["preview:x", "commit:x"]);
    }
}
