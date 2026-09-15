//! Encoding transformed images back to bytes.
//!
//! [`encode`] renders a [`DynamicImage`] to the requested container. JPEG
//! honours the builder's `quality` (1..=100); the other codecs use their
//! default/lossless settings. Because the `image` encoder writes **no** EXIF,
//! metadata is inherently stripped on every re-encode — the builder's
//! `strip_metadata` flag therefore documents intent rather than toggling an
//! explicit copy step.

use image::{DynamicImage, ImageEncoder};

use crate::error::{ImageError, Result};
use crate::format::Format;

/// Encode `img` to `format`, applying `quality` where the codec supports it.
///
/// # Errors
///
/// [`ImageError::Encode`] when the codec rejects the image (e.g. an encoder
/// that does not accept the pixel layout).
pub(crate) fn encode(img: &DynamicImage, format: Format, quality: u8) -> Result<Vec<u8>> {
    match format {
        Format::Jpeg => encode_jpeg(img, quality),
        Format::Png => encode_png(img, quality),
        Format::Webp => encode_webp(img),
        Format::Gif | Format::Bmp | Format::Tiff => encode_default(img, format.to_image_format()),
    }
}

/// Encode JPEG at the given quality (1..=100).
fn encode_jpeg(img: &DynamicImage, quality: u8) -> Result<Vec<u8>> {
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width(), rgb.height());
    let mut out = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    encoder
        .write_image(rgb.as_raw(), w, h, image::ExtendedColorType::Rgb8)
        .map_err(|e| ImageError::Encode(e.to_string()))?;
    Ok(out)
}

/// Encode PNG. `quality` (1..=100) maps inversely onto the zlib compression
/// level so a higher quality keeps more data (larger, faster file).
fn encode_png(img: &DynamicImage, quality: u8) -> Result<Vec<u8>> {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    let level = (100u32.saturating_sub(quality as u32) * 9 / 99).min(9) as u8;
    let mut out = Vec::new();
    let encoder = PngEncoder::new_with_quality(
        &mut out,
        CompressionType::Level(level),
        FilterType::Adaptive,
    );
    encoder
        .write_image(rgba.as_raw(), w, h, image::ExtendedColorType::Rgba8)
        .map_err(|e| ImageError::Encode(e.to_string()))?;
    Ok(out)
}

/// Encode WebP losslessly (the `image` codec has no lossy encoder).
///
/// The source pixel layout is preserved: images carrying an alpha channel are
/// encoded as RGBA (so transparency survives the round-trip), while opaque
/// images use RGB to avoid needless payload.
fn encode_webp(img: &DynamicImage) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let encoder = image::codecs::webp::WebPEncoder::new_lossless(&mut out);
    if img.color().has_alpha() {
        let rgba = img.to_rgba8();
        let (w, h) = (rgba.width(), rgba.height());
        encoder
            .write_image(rgba.as_raw(), w, h, image::ExtendedColorType::Rgba8)
            .map_err(|e| ImageError::Encode(e.to_string()))?;
    } else {
        let rgb = img.to_rgb8();
        let (w, h) = (rgb.width(), rgb.height());
        encoder
            .write_image(rgb.as_raw(), w, h, image::ExtendedColorType::Rgb8)
            .map_err(|e| ImageError::Encode(e.to_string()))?;
    }
    Ok(out)
}

/// Encode via the default `DynamicImage::write_to` path (GIF/BMP/TIFF).
fn encode_default(img: &DynamicImage, format: image::ImageFormat) -> Result<Vec<u8>> {
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, format)
        .map_err(|e| ImageError::Encode(e.to_string()))?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A JPEG encodes with a valid SOI marker and decodes back.
    #[test]
    fn jpeg_round_trip() {
        let img = crate::testutil::decode_png(&crate::testutil::gradient_png(32, 24));
        let bytes = encode(&img, Format::Jpeg, 90).unwrap();
        assert_eq!(&bytes[0..2], &[0xFF, 0xD8]);
        let back = crate::load::decode(&bytes).unwrap();
        assert_eq!(crate::load::dims(&back), (32, 24));
    }

    /// PNG and WebP round-trip through their encoders.
    #[test]
    fn png_and_webp_round_trip() {
        let img = crate::testutil::decode_png(&crate::testutil::gradient_png(20, 10));
        let png = encode(&img, Format::Png, 80).unwrap();
        assert_eq!(&png[0..4], &[0x89, b'P', b'N', b'G']);
        let webp = encode(&img, Format::Webp, 80).unwrap();
        assert_eq!(&webp[0..4], b"RIFF");
        assert_eq!(
            crate::load::dims(&crate::load::decode(&webp).unwrap()),
            (20, 10)
        );
    }

    /// Alpha survives a WebP round-trip: the transparent corner stays
    /// transparent and the opaque pixels stay opaque.
    #[test]
    fn webp_preserves_alpha() {
        let img = crate::testutil::decode_png(&crate::testutil::transparent_png(4, 4));
        assert!(img.color().has_alpha());
        let webp = encode(&img, Format::Webp, 80).unwrap();
        let back = crate::load::decode(&webp).unwrap();
        assert!(back.color().has_alpha());
        let rgba = back.to_rgba8();
        assert_eq!(rgba.get_pixel(0, 0).0[3], 0, "transparent pixel lost alpha");
        assert_eq!(
            rgba.get_pixel(3, 3).0[3],
            255,
            "opaque pixel became transparent"
        );
    }

    /// An opaque image still encodes to WebP and round-trips.
    #[test]
    fn webp_opaque_path_unchanged() {
        let img = crate::testutil::decode_png(&crate::testutil::gradient_png(8, 8));
        assert!(!img.color().has_alpha());
        let webp = encode(&img, Format::Webp, 80).unwrap();
        assert_eq!(&webp[0..4], b"RIFF");
        assert_eq!(
            crate::load::dims(&crate::load::decode(&webp).unwrap()),
            (8, 8)
        );
    }
}
