//! γ — effective autocorrect ruleset: built-in smart-typography
//! preset prepended to user rules.
//!
//! Carved out of `settings.rs` so that file stays under the 600-line
//! cap. The single helper lives on [`Settings`] via a sibling
//! `impl` block.

use crate::autocorrect::AutocorrectRule;
use crate::smart_typography::smart_typography_rules;
use crate::Settings;

impl Settings {
    /// Effective autocorrect rule list: built-in smart-typography
    /// preset prepended to user rules when the preset toggle is on.
    /// User rules win on identical patterns because the engine's
    /// [`first_match`] stops on the first applicable rule, so a
    /// user-defined entry for the same pattern listed *after* the
    /// preset will only fire when the preset rule itself rejects
    /// (e.g. fails the word-boundary check) — which is the right
    /// "user override" semantics.
    ///
    /// [`first_match`]: crate::autocorrect::first_match
    #[must_use]
    pub fn effective_autocorrect_rules(
        &self,
        user_rules: &[AutocorrectRule],
    ) -> Vec<AutocorrectRule> {
        let mut out = Vec::new();
        if self.editor.smart_typography_enabled {
            out.extend(smart_typography_rules());
        }
        out.extend(user_rules.iter().cloned());
        out
    }
}
