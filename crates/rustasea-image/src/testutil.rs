//! Test-only helpers for generating image fixtures in memory.
//!
//! The crate ships **no** binary fixtures; every test builds its inputs with
//! the `image` encoder. This module adds the one thing the encoder cannot do:
//! splice a minimal EXIF APP1 segment (carrying only the orientation tag) into
//! a JPEG so the auto-orient path can be exercised.

#![cfg(test)]

use image::{DynamicImage, ImageFormat, RgbImage, RgbaImage};

/// Encode a solid `w`×`h` RGB image as JPEG bytes.
pub(crate) fn jpeg_bytes(w: u32, h: u32) -> Vec<u8> {
    let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([128, 128, 128])));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Jpeg)
        .expect("encode jpeg");
    out.into_inner()
}

/// Encode a `w`×`h` PNG whose pixels vary with position (so JPEG re-encoding at
/// a low quality produces a measurably different size than at a high quality).
pub(crate) fn gradient_png(w: u32, h: u32) -> Vec<u8> {
    let img = RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8])
    });
    let mut out = std::io::Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(img)
        .write_to(&mut out, ImageFormat::Png)
        .expect("encode png");
    out.into_inner()
}

/// Decode PNG bytes back into a `DynamicImage` (test convenience).
pub(crate) fn decode_png(bytes: &[u8]) -> DynamicImage {
    image::load_from_memory_with_format(bytes, ImageFormat::Png).expect("decode png")
}

/// Encode a `w`×`h` RGBA PNG where the top-left pixel is fully transparent
/// (alpha `0`) and every other pixel is an opaque red.
///
/// Used to verify that alpha survives a lossless re-encode.
pub(crate) fn transparent_png(w: u32, h: u32) -> Vec<u8> {
    let img = RgbaImage::from_fn(w, h, |x, y| {
        if x == 0 && y == 0 {
            image::Rgba([255, 0, 0, 0])
        } else {
            image::Rgba([255, 0, 0, 255])
        }
    });
    let mut out = std::io::Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(img)
        .write_to(&mut out, ImageFormat::Png)
        .expect("encode transparent png");
    out.into_inner()
}

/// Build a structurally valid PNG whose IHDR declares `w`×`h` without any
/// actual pixel data.
///
/// Used to exercise the decoder's resource limits: the header advertises
/// enormous dimensions, so a bounded decoder must reject it from the header
/// alone — before allocating a pixel buffer. The IHDR chunk carries a correct
/// CRC so the failure is attributable to the limits, not a corrupt header.
pub(crate) fn png_with_declared_dims(w: u32, h: u32) -> Vec<u8> {
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // bit depth 8, truecolor, defaults

    let mut out = Vec::new();
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    out.extend_from_slice(&(ihdr.len() as u32).to_be_bytes());
    out.extend_from_slice(b"IHDR");
    out.extend_from_slice(&ihdr);
    out.extend_from_slice(&crc32(&[b"IHDR".as_slice(), ihdr.as_slice()].concat()).to_be_bytes());
    // IEND (empty), so the stream is well-formed up to the point of failure.
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(b"IEND");
    out.extend_from_slice(&crc32(b"IEND").to_be_bytes());
    out
}

/// CRC-32 (IEEE 802.3) as used by PNG chunk trailers.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Build a JPEG whose APP1 `Exif` block carries `orientation`.
///
/// `little` selects the TIFF byte order (`II` when true, `MM` when false). The
/// minimal TIFF block contains a single IFD0 entry — tag `0x0112`
/// (`Orientation`), type SHORT, count 1 — followed by a zero next-IFD offset.
pub(crate) fn jpeg_with_exif_orientation(orientation: u8, little: bool) -> Vec<u8> {
    let tiff = tiff_orientation_block(orientation, little);

    // APP1 segment: FF E1 | length(2, BE, incl. itself) | "Exif\0\0" | TIFF.
    let payload_len = 6 + tiff.len();
    let seg_len = (payload_len + 2) as u16;
    let mut app1 = Vec::with_capacity(payload_len + 4);
    app1.extend_from_slice(&[0xFF, 0xE1]);
    app1.extend_from_slice(&seg_len.to_be_bytes());
    app1.extend_from_slice(b"Exif\0\0");
    app1.extend_from_slice(&tiff);

    // Splice the APP1 segment immediately after the SOI marker.
    let base = jpeg_bytes(64, 48);
    let mut out = Vec::with_capacity(base.len() + app1.len());
    out.extend_from_slice(&base[0..2]); // FF D8 (SOI)
    out.extend_from_slice(&app1);
    out.extend_from_slice(&base[2..]);
    out
}

/// A 26-byte TIFF block with one IFD0 orientation entry.
fn tiff_orientation_block(orientation: u8, little: bool) -> Vec<u8> {
    let mut b = Vec::with_capacity(26);
    if little {
        b.extend_from_slice(b"II");
        b.extend_from_slice(&42u16.to_le_bytes());
        b.extend_from_slice(&8u32.to_le_bytes()); // IFD0 offset
        b.extend_from_slice(&1u16.to_le_bytes()); // entry count
        b.extend_from_slice(&0x0112u16.to_le_bytes()); // tag
        b.extend_from_slice(&3u16.to_le_bytes()); // type SHORT
        b.extend_from_slice(&1u32.to_le_bytes()); // count
        b.extend_from_slice(&[orientation, 0, 0, 0]); // value (first 2 bytes)
        b.extend_from_slice(&0u32.to_le_bytes()); // next IFD
    } else {
        b.extend_from_slice(b"MM");
        b.extend_from_slice(&42u16.to_be_bytes());
        b.extend_from_slice(&8u32.to_be_bytes());
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&0x0112u16.to_be_bytes());
        b.extend_from_slice(&3u16.to_be_bytes());
        b.extend_from_slice(&1u32.to_be_bytes());
        b.extend_from_slice(&[0, orientation, 0, 0]);
        b.extend_from_slice(&0u32.to_be_bytes());
    }
    b
}
