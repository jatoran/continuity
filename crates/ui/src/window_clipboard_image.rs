//! Phase F5 — clipboard image consumer.
//!
//! Two surfaces:
//!
//! 1. Pure functions [`decode_dib_to_rgba`] and [`encode_rgba_to_png`]
//!    convert a Win32 clipboard `CF_DIB` blob into a PNG-encoded byte
//!    vector. They live here (not in the `win` crate) because the
//!    PNG encoder is a UI-layer dependency.
//! 2. [`Window::try_paste_clipboard_image`] orchestrates the full
//!    paste path: probe the clipboard, decode / re-encode if needed,
//!    hand the bytes to [`crate::image_store::import_bytes`], and
//!    insert the markdown reference via `SelectionEdit::InsertText`.
//!    Returns `true` when an image branch was consumed (the caller
//!    falls through to text only on `false`).
//!
//! Thread ownership: UI thread (clipboard reads are HWND-bound).

use std::fs;
use std::path::Path;

use continuity_core::SelectionEdit;
use continuity_win::clipboard_image::{read_dib_bytes, read_dropped_image_paths};

use crate::image_store::{import_bytes, is_supported_image_extension};
use crate::Window;

/// Decoded top-down 8-bit-per-channel RGBA image. The painter and the
/// PNG encoder both consume this shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbaImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes, top-down rows, R G B A per pixel.
    pub rgba: Vec<u8>,
}

/// Errors the DIB decoder can return. Designed so the orchestrator
/// can decide whether to fall through to the text path or surface a
/// banner — every variant means "the clipboard had image-shaped
/// bytes but we cannot ingest them right now".
#[derive(Debug, thiserror::Error)]
pub enum ImagePasteError {
    /// The blob is shorter than a `BITMAPINFOHEADER`, or the declared
    /// header size points past the buffer.
    #[error("clipboard DIB header truncated or invalid")]
    BadHeader,
    /// Pixel format not on the supported set (16 / 24 / 32 bpp with
    /// BI_RGB or BI_BITFIELDS). Paletted formats fall here.
    #[error(
        "clipboard DIB uses an unsupported pixel format ({bpp} bpp, compression {compression})"
    )]
    UnsupportedFormat {
        /// `biBitCount`.
        bpp: u16,
        /// `biCompression`.
        compression: u32,
    },
    /// The computed pixel-row range walks past the end of the blob.
    #[error("clipboard DIB pixel data truncated")]
    PixelDataTruncated,
    /// The PNG encoder rejected the rgba buffer (shouldn't happen
    /// for well-formed RGBA8).
    #[error("png encode: {0}")]
    PngEncode(String),
}

/// Decode a `CF_DIB` / `CF_DIBV5` blob into top-down RGBA. Supports
/// the formats Windows clipboards typically populate:
///
/// * 24bpp BI_RGB (Snip & Sketch, classic Paint, most screenshot apps)
/// * 32bpp BI_RGB (alpha-aware copies)
/// * 32bpp BI_BITFIELDS with the canonical BGRA masks
///
/// Paletted (≤8bpp) and JPEG/PNG-compressed DIB variants return
/// [`ImagePasteError::UnsupportedFormat`].
///
/// # Errors
///
/// See [`ImagePasteError`].
pub fn decode_dib_to_rgba(blob: &[u8]) -> Result<RgbaImage, ImagePasteError> {
    if blob.len() < 40 {
        return Err(ImagePasteError::BadHeader);
    }
    let header_size = u32::from_le_bytes([blob[0], blob[1], blob[2], blob[3]]) as usize;
    if header_size < 40 || header_size > blob.len() {
        return Err(ImagePasteError::BadHeader);
    }
    let width = i32::from_le_bytes([blob[4], blob[5], blob[6], blob[7]]);
    let height_raw = i32::from_le_bytes([blob[8], blob[9], blob[10], blob[11]]);
    let bpp = u16::from_le_bytes([blob[14], blob[15]]);
    let compression = u32::from_le_bytes([blob[16], blob[17], blob[18], blob[19]]);
    if width <= 0 || height_raw == 0 {
        return Err(ImagePasteError::BadHeader);
    }
    if bpp != 24 && bpp != 32 {
        return Err(ImagePasteError::UnsupportedFormat { bpp, compression });
    }
    let height_abs = height_raw.unsigned_abs();
    let bottom_up = height_raw > 0;

    let mut pixel_offset = header_size;
    if compression == 3 {
        // BI_BITFIELDS — three DWORDs of channel masks follow the
        // 40-byte BITMAPINFOHEADER. Only present for V1 headers; V5
        // already encodes the masks inside the header. We do not
        // honour custom masks today (would require a full bit-shuffle
        // path); only the canonical BGRA layout below is supported.
        if header_size == 40 {
            pixel_offset += 12;
        }
    } else if compression != 0 {
        return Err(ImagePasteError::UnsupportedFormat { bpp, compression });
    }

    let row_bytes = ((width as usize) * (bpp as usize)).div_ceil(32) * 4;
    let total_pixel_bytes = row_bytes
        .checked_mul(height_abs as usize)
        .ok_or(ImagePasteError::BadHeader)?;
    if pixel_offset + total_pixel_bytes > blob.len() {
        return Err(ImagePasteError::PixelDataTruncated);
    }

    let pixels = &blob[pixel_offset..pixel_offset + total_pixel_bytes];
    let w = width as usize;
    let h = height_abs as usize;
    let mut rgba = vec![0u8; w * h * 4];

    for row in 0..h {
        let src_row = if bottom_up { h - 1 - row } else { row };
        let src_start = src_row * row_bytes;
        let src = &pixels[src_start..src_start + row_bytes];
        let dst_start = row * w * 4;
        match bpp {
            24 => {
                for col in 0..w {
                    let s = col * 3;
                    let d = dst_start + col * 4;
                    rgba[d] = src[s + 2];
                    rgba[d + 1] = src[s + 1];
                    rgba[d + 2] = src[s];
                    rgba[d + 3] = 0xff;
                }
            }
            32 => {
                for col in 0..w {
                    let s = col * 4;
                    let d = dst_start + col * 4;
                    rgba[d] = src[s + 2];
                    rgba[d + 1] = src[s + 1];
                    rgba[d + 2] = src[s];
                    // BI_RGB 32-bit DIBs nominally have an unused
                    // alpha byte; many writers leave it zero. Treat
                    // 0x00 as fully opaque (the common case) but
                    // honour any non-zero alpha for screenshots that
                    // legitimately encode transparency.
                    let a = src[s + 3];
                    rgba[d + 3] = if a == 0 { 0xff } else { a };
                }
            }
            _ => {
                return Err(ImagePasteError::UnsupportedFormat { bpp, compression });
            }
        }
    }

    Ok(RgbaImage {
        width: width as u32,
        height: height_abs,
        rgba,
    })
}

/// PNG-encode `img` (8-bit RGBA, top-down). Uses the `png` crate's
/// default zlib settings; the output is a complete PNG byte stream.
///
/// # Errors
///
/// Wraps any encoder failure into [`ImagePasteError::PngEncode`].
pub fn encode_rgba_to_png(img: &RgbaImage) -> Result<Vec<u8>, ImagePasteError> {
    let mut buf = Vec::with_capacity(img.rgba.len() / 4);
    {
        let mut encoder = png::Encoder::new(&mut buf, img.width, img.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| ImagePasteError::PngEncode(e.to_string()))?;
        writer
            .write_image_data(&img.rgba)
            .map_err(|e| ImagePasteError::PngEncode(e.to_string()))?;
    }
    Ok(buf)
}

impl Window {
    /// Try the image-paste branches before falling through to the
    /// text path. Returns `Ok(true)` when an image was successfully
    /// pasted (caller should NOT also paste text), `Ok(false)` when
    /// no image-shaped data is on the clipboard, and `Err(_)` when
    /// image-shaped data was found but the import failed (caller
    /// should also stop — the banner already names the failure).
    ///
    /// Probe order:
    /// 1. `CF_DIB` / `CF_DIBV5` — clipboard image bytes from Snip &
    ///    Sketch, Paint, screenshot tools, browser copy-image.
    /// 2. `CF_HDROP` — Explorer "Copy" on one or more files; if all
    ///    paths have image extensions we route through the same
    ///    import path as drag-drop.
    pub(crate) fn try_paste_clipboard_image(&mut self) -> Result<bool, ImagePasteError> {
        let images_dir = match self.image_store_dir.as_ref() {
            Some(d) => d.clone(),
            None => return Ok(false),
        };

        if let Some(blob) = read_dib_bytes(self.hwnd).ok().flatten() {
            let rgba = decode_dib_to_rgba(&blob)?;
            let png_bytes = encode_rgba_to_png(&rgba)?;
            self.insert_imported_image_bytes(&png_bytes, "png", &images_dir);
            return Ok(true);
        }

        let dropped = read_dropped_image_paths(self.hwnd).unwrap_or_default();
        let image_paths: Vec<_> = dropped
            .into_iter()
            .filter(|p| extension_is_image(p))
            .collect();
        if image_paths.is_empty() {
            return Ok(false);
        }
        for path in image_paths {
            let Ok(bytes) = fs::read(&path) else {
                continue;
            };
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            self.insert_imported_image_bytes(&bytes, ext, &images_dir);
        }
        Ok(true)
    }

    fn insert_imported_image_bytes(&mut self, bytes: &[u8], ext: &str, images_dir: &Path) {
        match import_bytes(bytes, ext, images_dir) {
            Ok(imported) => {
                let markdown = format!("![]({})", imported.markdown_reference);
                let _ = self
                    .editor
                    .apply_selection_edit(self.buffer_id, SelectionEdit::InsertText(markdown));
            }
            Err(err) => {
                self.file_banner = Some(crate::window_file::FileBanner::new(format!(
                    "Image paste failed: {err}"
                )));
            }
        }
    }
}

fn extension_is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(is_supported_image_extension)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal 24bpp BI_RGB DIB blob: 2×1 image, BGR pixels
    /// followed by a single padding byte so the row stride is the
    /// required 4-byte multiple (3 bytes × 2 cols = 6, padded to 8).
    fn synthetic_dib_24bpp(width: u32, height: i32, pixels_bgr: &[u8]) -> Vec<u8> {
        let mut blob = Vec::new();
        blob.extend_from_slice(&40u32.to_le_bytes()); // biSize
        blob.extend_from_slice(&(width as i32).to_le_bytes()); // biWidth
        blob.extend_from_slice(&height.to_le_bytes()); // biHeight (signed)
        blob.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
        blob.extend_from_slice(&24u16.to_le_bytes()); // biBitCount
        blob.extend_from_slice(&0u32.to_le_bytes()); // biCompression = BI_RGB
        blob.extend_from_slice(&0u32.to_le_bytes()); // biSizeImage
        blob.extend_from_slice(&2835u32.to_le_bytes()); // biXPelsPerMeter
        blob.extend_from_slice(&2835u32.to_le_bytes()); // biYPelsPerMeter
        blob.extend_from_slice(&0u32.to_le_bytes()); // biClrUsed
        blob.extend_from_slice(&0u32.to_le_bytes()); // biClrImportant
        blob.extend_from_slice(pixels_bgr);
        blob
    }

    #[test]
    fn decode_24bpp_bottom_up_flips_rows_and_swaps_bgr() {
        // 2×2 image, bottom-up. Source row 0 (bottom) is red+green,
        // source row 1 (top) is blue+white. Each row 4-byte padded.
        let mut pixels = Vec::new();
        // row 0 (bottom): R(0,0,FF) G(0,FF,0) then 2 pad bytes
        pixels.extend_from_slice(&[0x00, 0x00, 0xff, 0x00, 0xff, 0x00, 0x00, 0x00]);
        // row 1 (top): B(FF,0,0) W(FF,FF,FF) then 2 pad bytes
        pixels.extend_from_slice(&[0xff, 0x00, 0x00, 0xff, 0xff, 0xff, 0x00, 0x00]);
        let blob = synthetic_dib_24bpp(2, 2, &pixels);
        let img = decode_dib_to_rgba(&blob).expect("decode");
        assert_eq!(img.width, 2);
        assert_eq!(img.height, 2);
        // Output is top-down: first row should match the source's
        // *last* row, with BGR → RGB applied.
        assert_eq!(img.rgba[0..4], [0x00, 0x00, 0xff, 0xff]); // B → blue (R,G,B,A)
        assert_eq!(img.rgba[4..8], [0xff, 0xff, 0xff, 0xff]); // W → white
        assert_eq!(img.rgba[8..12], [0xff, 0x00, 0x00, 0xff]); // R → red
        assert_eq!(img.rgba[12..16], [0x00, 0xff, 0x00, 0xff]); // G → green
    }

    #[test]
    fn decode_24bpp_top_down_keeps_row_order() {
        // 2×1, top-down (negative height).
        let pixels = [0x00, 0x00, 0xff, 0x00, 0xff, 0x00, 0x00, 0x00];
        let blob = synthetic_dib_24bpp(2, -1, &pixels);
        let img = decode_dib_to_rgba(&blob).expect("decode");
        assert_eq!(img.width, 2);
        assert_eq!(img.height, 1);
        assert_eq!(img.rgba[0..4], [0xff, 0x00, 0x00, 0xff]);
        assert_eq!(img.rgba[4..8], [0x00, 0xff, 0x00, 0xff]);
    }

    #[test]
    fn decode_rejects_truncated_header() {
        let blob = vec![0u8; 20];
        assert!(matches!(
            decode_dib_to_rgba(&blob),
            Err(ImagePasteError::BadHeader)
        ));
    }

    #[test]
    fn decode_rejects_unsupported_bpp() {
        let mut blob = Vec::new();
        blob.extend_from_slice(&40u32.to_le_bytes());
        blob.extend_from_slice(&1i32.to_le_bytes());
        blob.extend_from_slice(&1i32.to_le_bytes());
        blob.extend_from_slice(&1u16.to_le_bytes());
        blob.extend_from_slice(&8u16.to_le_bytes()); // 8bpp paletted
        blob.extend_from_slice(&0u32.to_le_bytes());
        blob.extend_from_slice(&[0u8; 20]); // rest of header + padding
        match decode_dib_to_rgba(&blob) {
            Err(ImagePasteError::UnsupportedFormat { bpp, .. }) => assert_eq!(bpp, 8),
            other => panic!("expected UnsupportedFormat, got {other:?}"),
        }
    }

    #[test]
    fn decode_rejects_truncated_pixels() {
        let blob = synthetic_dib_24bpp(2, 1, &[]); // claims 2x1 24bpp but no pixels
        assert!(matches!(
            decode_dib_to_rgba(&blob),
            Err(ImagePasteError::PixelDataTruncated)
        ));
    }

    #[test]
    fn encode_round_trips_through_png() {
        let rgba = RgbaImage {
            width: 2,
            height: 2,
            rgba: vec![
                0xff, 0x00, 0x00, 0xff, 0x00, 0xff, 0x00, 0xff, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff,
                0xff, 0xff,
            ],
        };
        let png_bytes = encode_rgba_to_png(&rgba).expect("encode");
        // PNG signature.
        assert_eq!(
            &png_bytes[0..8],
            &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]
        );
        // Some payload past the signature.
        assert!(png_bytes.len() > 32);
    }

    #[test]
    fn full_clipboard_image_pipeline_round_trips() {
        // Synthetic DIB → decoded RGBA → PNG bytes → image_store
        // import. Asserts the markdown reference shape and the file
        // existence end-to-end (no HWND involved).
        let mut pixels = Vec::new();
        pixels.extend_from_slice(&[0xff, 0x00, 0x00, 0x00, 0xff, 0x00, 0x00, 0x00]);
        let blob = synthetic_dib_24bpp(2, -1, &pixels);
        let rgba = decode_dib_to_rgba(&blob).expect("decode");
        let png_bytes = encode_rgba_to_png(&rgba).expect("encode");

        let dir = tempfile::tempdir().expect("tempdir");
        let first = import_bytes(&png_bytes, "png", dir.path()).expect("import");
        assert!(first.was_written);
        assert!(first.markdown_reference.starts_with("images/"));
        assert!(first.markdown_reference.ends_with(".png"));

        // Idempotent on a second paste of the identical clipboard.
        let second = import_bytes(&png_bytes, "png", dir.path()).expect("import-2");
        assert!(!second.was_written);
        assert_eq!(first.markdown_reference, second.markdown_reference);
    }
}
