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
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    VK_BACK, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_F24, VK_HOME, VK_INSERT, VK_LEFT,
    VK_NEXT, VK_OEM_1, VK_OEM_2, VK_OEM_3, VK_OEM_4, VK_OEM_5, VK_OEM_6, VK_OEM_7, VK_OEM_COMMA,
    VK_OEM_MINUS, VK_OEM_PERIOD, VK_OEM_PLUS, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SPACE, VK_TAB,
    VK_UP,
};

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

    /// Construct a chord from a Win32 virtual-key code plus active modifiers.
    ///
    /// Returns `None` for keys that are not currently representable in the
    /// keymap grammar (including modifier-only keys).
    #[must_use]
    pub fn from_vk_modifiers(vk: u16, modifiers: Modifiers) -> Option<Self> {
        let key = key_name_from_vk(vk)?;
        Some(Self::new(modifiers, key))
    }
}

fn key_name_from_vk(vk: u16) -> Option<String> {
    match vk {
        0x30..=0x39 => Some(char::from_u32(u32::from(vk))?.to_string()),
        0x41..=0x5a => Some(
            char::from_u32(u32::from(vk))?
                .to_ascii_lowercase()
                .to_string(),
        ),
        VK_BACK => Some("backspace".into()),
        VK_DELETE => Some("delete".into()),
        VK_DOWN => Some("down".into()),
        VK_END => Some("end".into()),
        VK_ESCAPE => Some("escape".into()),
        VK_HOME => Some("home".into()),
        VK_INSERT => Some("insert".into()),
        VK_LEFT => Some("left".into()),
        VK_NEXT => Some("pagedown".into()),
        VK_PRIOR => Some("pageup".into()),
        VK_RETURN => Some("enter".into()),
        VK_RIGHT => Some("right".into()),
        VK_SPACE => Some("space".into()),
        VK_TAB => Some("tab".into()),
        VK_UP => Some("up".into()),
        VK_F1..=VK_F24 => Some(format!("f{}", vk - VK_F1 + 1)),
        // US-layout punctuation keys. Matches the unshifted label so
        // chord strings written as `ctrl+/` / `ctrl+[` / etc.
        // resolve correctly. Non-US layouts may produce different
        // glyphs for these scan codes — we accept that today because
        // the bundled keymap is US-only.
        VK_OEM_1 => Some(";".into()),
        VK_OEM_PLUS => Some("=".into()),
        VK_OEM_COMMA => Some(",".into()),
        VK_OEM_MINUS => Some("-".into()),
        VK_OEM_PERIOD => Some(".".into()),
        VK_OEM_2 => Some("/".into()),
        VK_OEM_3 => Some("`".into()),
        VK_OEM_4 => Some("[".into()),
        VK_OEM_5 => Some("\\".into()),
        VK_OEM_6 => Some("]".into()),
        VK_OEM_7 => Some("'".into()),
        _ => None,
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
    fn vk_to_key_handles_oem_punctuation() {
        // §H3 bug: `ctrl+k ctrl+/` and friends didn't fire because
        // `key_name_from_vk` only mapped letters / digits / arrows.
        // VK_OEM_2 = 0xBF (`/`), VK_OEM_4 = 0xDB (`[`), VK_OEM_5 =
        // 0xDC (`\`), VK_OEM_6 = 0xDD (`]`).
        let slash =
            KeyChord::from_vk_modifiers(0xBF, Modifiers::default()).expect("VK_OEM_2 mapped");
        assert_eq!(slash.key, "/");
        let lbracket =
            KeyChord::from_vk_modifiers(0xDB, Modifiers::default()).expect("VK_OEM_4 mapped");
        assert_eq!(lbracket.key, "[");
        let backslash =
            KeyChord::from_vk_modifiers(0xDC, Modifiers::default()).expect("VK_OEM_5 mapped");
        assert_eq!(backslash.key, "\\");
        let rbracket =
            KeyChord::from_vk_modifiers(0xDD, Modifiers::default()).expect("VK_OEM_6 mapped");
        assert_eq!(rbracket.key, "]");
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

    #[test]
    fn maps_win32_virtual_keys() {
        assert_eq!(
            KeyChord::from_vk_modifiers(VK_LEFT, Modifiers::default())
                .unwrap()
                .to_string(),
            "left"
        );
        assert_eq!(
            KeyChord::from_vk_modifiers(
                0x41,
                Modifiers {
                    ctrl: true,
                    ..Modifiers::default()
                },
            )
            .unwrap()
            .to_string(),
            "ctrl+a"
        );
        assert_eq!(
            KeyChord::from_vk_modifiers(VK_F1 + 11, Modifiers::default())
                .unwrap()
                .to_string(),
            "f12"
        );
    }
}
