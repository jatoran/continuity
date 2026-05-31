//! `FindContext` — the find-bar mutation surface, factored out of
//! [`crate::Context`] so the latter stays under the 600-line cap.
//!
//! Every method has a default `Err(UnsupportedContext("…"))` body so a
//! test stub can implement only what it exercises. The production impl
//! lives on `ui::Window`; the registry's `Handler` signature still uses
//! `&mut dyn Context`, which re-exposes these methods via supertrait
//! inheritance.

use crate::view_context::ViewContext;
use crate::Error;

/// Find-bar mutation surface (supertrait of [`crate::Context`]).
pub trait FindContext: ViewContext {
    /// Step the find-bar match cursor by `delta` (negative = previous).
    /// Wraps; no-op when there is no active find bar.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn find_step(&mut self, _delta: i32) -> Result<(), Error> {
        Err(Error::UnsupportedContext("find_step"))
    }

    /// Replace the currently-highlighted find match with the bar's
    /// replace text and step to the next match.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn find_replace_one(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("find_replace_one"))
    }

    /// Replace every find match in the current buffer as one undo group.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn find_replace_all(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("find_replace_all"))
    }

    /// G1 — flip one of the find-bar mode toggles. `mode` is one of
    /// `"case"`, `"word"`, `"regex"`, `"preserve"`, `"scope"`.
    /// Unknown modes are implementation-defined; the default surface
    /// only returns `UnsupportedContext`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn find_toggle(&mut self, _mode: &str) -> Result<(), Error> {
        Err(Error::UnsupportedContext("find_toggle"))
    }

    /// G3 — convert every find-bar match into a cursor (one selection
    /// per match) and dismiss the bar. No-op when no bar is open or
    /// the match set is empty.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn find_matches_to_cursors(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("find_matches_to_cursors"))
    }

    /// G3 — drop the primary cursor and advance to the next occurrence
    /// of the primary selection's text (Sublime Text–style skip).
    /// Operates on selection state, not the find-bar match list, so it
    /// works whether or not the bar is open.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedContext`] when unsupported.
    fn skip_current_match(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("skip_current_match"))
    }
}
