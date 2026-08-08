//! Win32 virtual-key translation into platform-neutral keymap chords.
//!
//! Physical key codes are a host-adapter concern. The keymap consumes only
//! [`continuity_input::KeyChord`], so other hosts can translate their own
//! keyboard event model without importing Win32 constants.

use continuity_input::{KeyChord, Modifiers};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VK_BACK, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_F24, VK_HOME, VK_INSERT, VK_LEFT,
    VK_NEXT, VK_OEM_1, VK_OEM_2, VK_OEM_3, VK_OEM_4, VK_OEM_5, VK_OEM_6, VK_OEM_7, VK_OEM_COMMA,
    VK_OEM_MINUS, VK_OEM_PERIOD, VK_OEM_PLUS, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SPACE, VK_TAB,
    VK_UP,
};

/// Translate one Win32 virtual key plus current modifiers into a keymap chord.
///
/// Returns `None` for modifier-only or unsupported virtual keys.
#[must_use]
pub(crate) fn key_chord_from_virtual_key(vk: u16, modifiers: Modifiers) -> Option<KeyChord> {
    let key = match vk {
        0x30..=0x39 => char::from_u32(u32::from(vk))?.to_string(),
        0x41..=0x5a => char::from_u32(u32::from(vk))?
            .to_ascii_lowercase()
            .to_string(),
        value if value == VK_BACK.0 => "backspace".into(),
        value if value == VK_DELETE.0 => "delete".into(),
        value if value == VK_DOWN.0 => "down".into(),
        value if value == VK_END.0 => "end".into(),
        value if value == VK_ESCAPE.0 => "escape".into(),
        value if value == VK_HOME.0 => "home".into(),
        value if value == VK_INSERT.0 => "insert".into(),
        value if value == VK_LEFT.0 => "left".into(),
        value if value == VK_NEXT.0 => "pagedown".into(),
        value if value == VK_PRIOR.0 => "pageup".into(),
        value if value == VK_RETURN.0 => "enter".into(),
        value if value == VK_RIGHT.0 => "right".into(),
        value if value == VK_SPACE.0 => "space".into(),
        value if value == VK_TAB.0 => "tab".into(),
        value if value == VK_UP.0 => "up".into(),
        value if (VK_F1.0..=VK_F24.0).contains(&value) => {
            format!("f{}", value - VK_F1.0 + 1)
        }
        value if value == VK_OEM_1.0 => ";".into(),
        value if value == VK_OEM_PLUS.0 => "=".into(),
        value if value == VK_OEM_COMMA.0 => ",".into(),
        value if value == VK_OEM_MINUS.0 => "-".into(),
        value if value == VK_OEM_PERIOD.0 => ".".into(),
        value if value == VK_OEM_2.0 => "/".into(),
        value if value == VK_OEM_3.0 => "`".into(),
        value if value == VK_OEM_4.0 => "[".into(),
        value if value == VK_OEM_5.0 => "\\".into(),
        value if value == VK_OEM_6.0 => "]".into(),
        value if value == VK_OEM_7.0 => "'".into(),
        _ => return None,
    };
    Some(KeyChord::new(modifiers, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_letters_named_keys_functions_and_punctuation() {
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        assert_eq!(
            key_chord_from_virtual_key(0x41, ctrl)
                .expect("A maps")
                .to_string(),
            "ctrl+a"
        );
        assert_eq!(
            key_chord_from_virtual_key(VK_LEFT.0, Modifiers::default())
                .expect("left maps")
                .key,
            "left"
        );
        assert_eq!(
            key_chord_from_virtual_key(VK_F1.0 + 11, Modifiers::default())
                .expect("F12 maps")
                .key,
            "f12"
        );
        assert_eq!(
            key_chord_from_virtual_key(VK_OEM_2.0, ctrl)
                .expect("slash maps")
                .to_string(),
            "ctrl+/"
        );
    }

    #[test]
    fn ignores_modifier_only_virtual_keys() {
        assert!(key_chord_from_virtual_key(0x10, Modifiers::default()).is_none());
    }
}
