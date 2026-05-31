//! 32-bit RGBA colors and `#rrggbb`/`#rrggbbaa` parsing.

use std::str::FromStr;

use serde::{Deserialize, Deserializer};

use crate::Error;

/// 8-bit-per-channel sRGB color with alpha.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Color {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
    /// Alpha (255 = opaque).
    pub a: u8,
}

impl Color {
    /// Construct from RGBA components.
    #[must_use]
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Construct from RGB components, fully opaque.
    #[must_use]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }
}

impl FromStr for Color {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let hex = s
            .strip_prefix('#')
            .ok_or_else(|| Error::InvalidColor(s.to_string()))?;
        let parse_pair = |i: usize| -> Result<u8, Error> {
            u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| Error::InvalidColor(s.to_string()))
        };
        match hex.len() {
            6 => Ok(Self::rgb(parse_pair(0)?, parse_pair(2)?, parse_pair(4)?)),
            8 => Ok(Self::rgba(
                parse_pair(0)?,
                parse_pair(2)?,
                parse_pair(4)?,
                parse_pair(6)?,
            )),
            _ => Err(Error::InvalidColor(s.to_string())),
        }
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_six_digit_hex() {
        let c: Color = "#ff8000".parse().unwrap();
        assert_eq!(c, Color::rgb(255, 128, 0));
    }

    #[test]
    fn parses_eight_digit_hex_with_alpha() {
        let c: Color = "#11223344".parse().unwrap();
        assert_eq!(c, Color::rgba(0x11, 0x22, 0x33, 0x44));
    }

    #[test]
    fn parses_uppercase() {
        let c: Color = "#ABCDEF".parse().unwrap();
        assert_eq!(c, Color::rgb(0xAB, 0xCD, 0xEF));
    }

    #[test]
    fn rejects_missing_hash() {
        assert!("ff8000".parse::<Color>().is_err());
    }

    #[test]
    fn rejects_wrong_length() {
        assert!("#abc".parse::<Color>().is_err());
        assert!("#abcdefab12".parse::<Color>().is_err());
    }

    #[test]
    fn rejects_non_hex() {
        assert!("#zzzzzz".parse::<Color>().is_err());
    }
}
