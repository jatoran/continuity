//! Checked UTF-8 byte and UTF-16 code-unit boundary conversion.

use crate::Error;

/// Convert a UTF-8 byte boundary to a UTF-16 code-unit offset.
///
/// # Errors
///
/// Returns [`Error::InvalidTextBoundary`] for out-of-range or interior UTF-8
/// byte offsets.
pub fn byte_to_utf16(text: &str, byte: usize) -> Result<u32, Error> {
    if byte > text.len() || !text.is_char_boundary(byte) {
        return Err(Error::InvalidTextBoundary(byte));
    }
    u32::try_from(text[..byte].encode_utf16().count()).map_err(|_| Error::InvalidTextBoundary(byte))
}

/// Convert a UTF-16 code-unit boundary to a UTF-8 byte offset.
///
/// # Errors
///
/// Returns [`Error::InvalidTextBoundary`] when the offset is beyond the
/// string or splits a surrogate pair.
pub fn utf16_to_byte(text: &str, utf16: u32) -> Result<usize, Error> {
    let target = utf16 as usize;
    let mut units = 0usize;
    for (byte, character) in text.char_indices() {
        if units == target {
            return Ok(byte);
        }
        let next = units + character.len_utf16();
        if target < next {
            return Err(Error::InvalidTextBoundary(target));
        }
        units = next;
    }
    if units == target {
        Ok(text.len())
    } else {
        Err(Error::InvalidTextBoundary(target))
    }
}
