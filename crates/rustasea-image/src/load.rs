//! Decoding and EXIF orientation handling.
//!
//! [`decode`] turns a byte slice into a `DynamicImage`, mapping any failure to
//! a typed [`ImageError::Decode`]. [`read_exif_orientation`] parses the JPEG
//! APP1 `Exif` block **by hand** (no extra dependency): it scans the segment
//! table, reads the TIFF header endianness, and looks for the orientation tag
//! (`0x0112`). Any structural surprise degrades to orientation `1` (no-op) —
//! EXIF parsing never turns a loadable image into an error. [`apply_orientation`]
//! then applies the corresponding `image` transform.

use std::io::Cursor;

use image::{DynamicImage, GenericImageView, ImageReader};

use crate::error::{ImageError, Result};
use crate::format::Format;

/// Maximum decoded width or height, in pixels, accepted by [`decode`].
const MAX_DIMENSION: u32 = 16_384;
/// Maximum total decoded allocation (256 MiB) accepted by [`decode`].
const MAX_ALLOC: u64 = 256 * 1024 * 1024;

/// Decode `bytes` into a [`DynamicImage`], best-effort across all codecs.
///
/// Decoding is bounded to defend against decompression bombs: the declared
/// image dimensions must not exceed `16_384` pixels on either axis and the
/// total decoder allocation must not exceed `256 MiB`. Inputs whose headers
/// advertise more than that are rejected before any pixels are allocated.
///
/// # Errors
///
/// [`ImageError::Decode`] when the bytes are not a decodable image or exceed
/// the resource limits above.
pub(crate) fn decode(bytes: &[u8]) -> Result<DynamicImage> {
    // `Limits` is `#[non_exhaustive]`, so start from `Default` and override the
    // fields we constrain (the default `max_alloc` is 512 MiB).
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_ALLOC);

    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| ImageError::Decode(e.to_string()))?;
    reader.limits(limits);
    reader
        .decode()
        .map_err(|e| ImageError::Decode(e.to_string()))
}

/// Guess the container format of `bytes` from its magic number.
///
/// Returns `None` when the signature is unknown or the format is not one the
/// crate advertises.
pub(crate) fn guess_format(bytes: &[u8]) -> Option<Format> {
    image::guess_format(bytes)
        .ok()
        .and_then(Format::from_image_format)
}

/// Apply an EXIF orientation code (`1..=8`) to `img`.
///
/// Values outside `1..=8` are treated as `1` (identity). The transforms mirror
/// the EXIF specification's pixel-mapping table.
pub(crate) fn apply_orientation(img: DynamicImage, orientation: u8) -> DynamicImage {
    use image::imageops::{flip_horizontal, flip_vertical, rotate180, rotate270, rotate90};
    // The `image` imageops helpers return an `ImageBuffer<Rgba<u8>, _>`, so each
    // result is rewrapped as a `DynamicImage`.
    let wrap = DynamicImage::ImageRgba8;
    match orientation {
        2 => wrap(flip_horizontal(&img)),
        3 => wrap(rotate180(&img)),
        4 => wrap(flip_vertical(&img)),
        5 => wrap(flip_horizontal(&rotate90(&img))),
        6 => wrap(rotate90(&img)),
        7 => wrap(flip_horizontal(&rotate270(&img))),
        8 => wrap(rotate270(&img)),
        _ => img,
    }
}

/// Read the EXIF orientation tag from a JPEG byte stream.
///
/// Returns `1` (the EXIF default, meaning "no transform") whenever the stream
/// is not a JPEG, has no `Exif` APP1 block, or is malformed at any point.
pub(crate) fn read_exif_orientation(bytes: &[u8]) -> u8 {
    read_tiff_orientation(bytes).unwrap_or(1)
}

/// Parse the orientation out of the JPEG `Exif` block, if present.
fn read_tiff_orientation(bytes: &[u8]) -> Option<u8> {
    // JPEG must start with the SOI marker FF D8.
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }
    let mut pos = 2usize;
    while pos + 4 <= bytes.len() {
        // Segment markers are 0xFF followed by a non-zero marker byte.
        if bytes[pos] != 0xFF {
            return None;
        }
        let marker = bytes[pos + 1];
        // Start-of-scan / end-of-image: no more metadata segments.
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        let len = u16::from_be_bytes([bytes[pos + 2], bytes[pos + 3]]) as usize;
        if len < 2 {
            return None;
        }
        let payload_start = pos + 4;
        let payload_end = pos + 2 + len;
        if payload_end > bytes.len() {
            return None;
        }
        if marker == 0xE1 {
            let payload = &bytes[payload_start..payload_end];
            if payload.starts_with(b"Exif\0\0") {
                return tiff_orientation(&payload[6..]);
            }
        }
        pos = payload_end;
    }
    None
}

/// Extract tag `0x0112` (Orientation) from a TIFF block (the body after
/// `Exif\0\0`). Handles both `II` (little-endian) and `MM` (big-endian) headers.
fn tiff_orientation(tiff: &[u8]) -> Option<u8> {
    if tiff.len() < 8 {
        return None;
    }
    let little = match &tiff[0..2] {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let read_u16 = |at: usize| -> Option<u16> {
        let raw = [*tiff.get(at)?, *tiff.get(at.checked_add(1)?)?];
        Some(if little {
            u16::from_le_bytes(raw)
        } else {
            u16::from_be_bytes(raw)
        })
    };
    let read_u32 = |at: usize| -> Option<u32> {
        let raw = [
            *tiff.get(at)?,
            *tiff.get(at.checked_add(1)?)?,
            *tiff.get(at.checked_add(2)?)?,
            *tiff.get(at.checked_add(3)?)?,
        ];
        Some(if little {
            u32::from_le_bytes(raw)
        } else {
            u32::from_be_bytes(raw)
        })
    };
    // TIFF magic 42, then the offset to IFD0.
    if read_u16(2)? != 42 {
        return None;
    }
    let ifd0 = read_u32(4)? as usize;
    let count = read_u16(ifd0)? as usize;
    for i in 0..count {
        // IFD0 header (2 bytes) + `i` 12-byte entries; all arithmetic is
        // checked so a crafted offset near `usize::MAX` degrades to `None`
        // instead of panicking on a 32-bit target.
        let entry = ifd0.checked_add(2)?.checked_add(i.checked_mul(12)?)?;
        let tag = read_u16(entry)?;
        if tag == 0x0112 {
            // Type SHORT (3): the value lives in the first two bytes of the
            // 4-byte value/offset field.
            return Some(read_u16(entry.checked_add(8)?)? as u8);
        }
    }
    None
}

/// Convenience: `(width, height)` of a decoded image.
pub(crate) fn dims(img: &DynamicImage) -> (u32, u32) {
    img.dimensions()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A non-JPEG stream yields the identity orientation.
    #[test]
    fn non_jpeg_is_identity() {
        assert_eq!(read_exif_orientation(b"not a jpeg"), 1);
        assert_eq!(read_exif_orientation(&[]), 1);
    }

    /// A hand-built little-endian Exif block yields its orientation tag.
    #[test]
    fn reads_little_endian_orientation() {
        let bytes = crate::testutil::jpeg_with_exif_orientation(6, true);
        assert_eq!(read_exif_orientation(&bytes), 6);
    }

    /// A hand-built big-endian Exif block yields its orientation tag.
    #[test]
    fn reads_big_endian_orientation() {
        let bytes = crate::testutil::jpeg_with_exif_orientation(3, false);
        assert_eq!(read_exif_orientation(&bytes), 3);
    }

    /// A normal small image still decodes under the resource limits.
    #[test]
    fn small_image_decodes_under_limits() {
        let bytes = crate::testutil::gradient_png(32, 24);
        let img = decode(&bytes).unwrap();
        assert_eq!(dims(&img), (32, 24));
    }

    /// A PNG header declaring an enormous size is rejected from the header
    /// alone — a typed error, never a multi-gigabyte allocation.
    #[test]
    fn oversized_dimensions_are_rejected() {
        let bytes = crate::testutil::png_with_declared_dims(100_000, 100_000);
        let err = decode(&bytes).unwrap_err();
        assert!(matches!(err, ImageError::Decode(_)), "got {err:?}");
    }

    /// A TIFF block whose IFD0 offset is near `u32::MAX` returns `None`
    /// (orientation 1 fallback) without overflowing the offset arithmetic.
    #[test]
    fn tiff_ifd0_overflow_returns_none() {
        // "II", magic 42, IFD0 offset = u32::MAX.
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II");
        tiff.extend_from_slice(&42u16.to_le_bytes());
        tiff.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(tiff_orientation(&tiff), None);
    }

    /// A TIFF block whose IFD0 offset is just past the buffer returns `None`.
    #[test]
    fn tiff_ifd0_past_end_returns_none() {
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"MM");
        tiff.extend_from_slice(&42u16.to_be_bytes());
        tiff.extend_from_slice(&(u32::MAX - 2).to_be_bytes());
        assert_eq!(tiff_orientation(&tiff), None);
    }
}
