//! Key-chord parsing.
//!
//! Chords use a small textual grammar:
//! `("ctrl"|"alt"|"shift"|"super") "+" ... "+" KEY`
//!
//! KEY is one of:
//! - a single character (case-insensitive: `a`, `Z`, `5`)
//! - a function key: `f1`..`f24`
//! - a named key: `up`, `down`, `left`, `right`, `home`, `end`,
//!   `pageup`, `pagedown`, `tab`, `enter`, `escape`, `space`,
//!   `backspace`, `delete`, `insert`

use std::fmt;
use std::str::FromStr;

use crate::Error;
/// Modifier-key bitset.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    /// Ctrl pressed.
    pub ctrl: bool,
    /// Alt pressed.
    pub alt: bool,
    /// Shift pressed.
    pub shift: bool,
    /// Super / Windows / Meta key pressed.
    pub meta: bool,
}

/// A single key chord: zero-or-more modifiers plus a key.
///
/// Keys are stored as a normalized lowercase string. Common values:
/// single-char keys (`"a"`, `"5"`), function keys (`"f1"`..`"f24"`),
/// and named keys (`"up"`, `"enter"`, etc.).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct KeyChord {
    /// Active modifiers.
    pub modifiers: Modifiers,
    /// Normalized key name.
    pub key: String,
}

impl KeyChord {
    /// Construct a chord from parts. The `key` is normalized to lowercase.
    pub fn new(modifiers: Modifiers, key: impl Into<String>) -> Self {
        Self {
            modifiers,
            key: key.into().to_ascii_lowercase(),
        }
    }
}

impl FromStr for KeyChord {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.trim().is_empty() {
            return Err(Error::InvalidChord(s.to_string()));
        }
        let mut modifiers = Modifiers::default();
        let mut key: Option<String> = None;
        for part in s.split('+') {
            let p = part.trim().to_ascii_lowercase();
            if p.is_empty() {
                return Err(Error::InvalidChord(s.to_string()));
            }
            match p.as_str() {
                "ctrl" | "control" => modifiers.ctrl = true,
                "alt" => modifiers.alt = true,
                "shift" => modifiers.shift = true,
                "super" | "win" | "meta" => modifiers.meta = true,
                other => {
                    if key.is_some() {
                        return Err(Error::InvalidChord(s.to_string()));
                    }
                    key = Some(other.to_string());
                }
            }
        }
        let key = key.ok_or_else(|| Error::InvalidChord(s.to_string()))?;
        Ok(Self { modifiers, key })
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        let mut sep = |f: &mut fmt::Formatter<'_>| -> fmt::Result {
            if !first {
                f.write_str("+")?;
            }
            first = false;
            Ok(())
        };
        if self.modifiers.ctrl {
            sep(f)?;
            f.write_str("ctrl")?;
        }
        if self.modifiers.alt {
            sep(f)?;
            f.write_str("alt")?;
        }
        if self.modifiers.shift {
            sep(f)?;
            f.write_str("shift")?;
        }
        if self.modifiers.meta {
            sep(f)?;
            f.write_str("super")?;
        }
        sep(f)?;
        f.write_str(&self.key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_key() {
        let c: KeyChord = "a".parse().unwrap();
        assert_eq!(c.key, "a");
        assert_eq!(c.modifiers, Modifiers::default());
    }

    #[test]
    fn parses_ctrl_letter() {
        let c: KeyChord = "ctrl+b".parse().unwrap();
        assert!(c.modifiers.ctrl);
        assert_eq!(c.key, "b");
    }

    #[test]
    fn parses_full_chord_case_insensitive() {
        let c: KeyChord = "Ctrl+Alt+Shift+Up".parse().unwrap();
        assert!(c.modifiers.ctrl);
        assert!(c.modifiers.alt);
        assert!(c.modifiers.shift);
        assert!(!c.modifiers.meta);
        assert_eq!(c.key, "up");
    }

    #[test]
    fn parses_super_aliases() {
        let a: KeyChord = "win+l".parse().unwrap();
        let b: KeyChord = "super+l".parse().unwrap();
        let m: KeyChord = "meta+l".parse().unwrap();
        assert!(a.modifiers.meta && b.modifiers.meta && m.modifiers.meta);
    }

    #[test]
    fn parses_function_keys() {
        let c: KeyChord = "f12".parse().unwrap();
        assert_eq!(c.key, "f12");
    }

    #[test]
    fn rejects_empty() {
        assert!("".parse::<KeyChord>().is_err());
    }

    #[test]
    fn rejects_modifiers_without_key() {
        assert!("ctrl+shift".parse::<KeyChord>().is_err());
    }

    #[test]
    fn rejects_two_keys() {
        assert!("ctrl+a+b".parse::<KeyChord>().is_err());
    }

    #[test]
    fn rejects_empty_part() {
        assert!("ctrl++a".parse::<KeyChord>().is_err());
    }

    #[test]
    fn round_trip_through_display() {
        for input in ["a", "ctrl+b", "ctrl+alt+shift+up", "f1", "shift+f12"] {
            let c: KeyChord = input.parse().unwrap();
            let s = c.to_string();
            let c2: KeyChord = s.parse().unwrap();
            assert_eq!(c, c2, "input: {input}, displayed: {s}");
        }
    }
}
