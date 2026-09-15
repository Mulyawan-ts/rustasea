//! Pipeline, encode, EXIF, and watermark integration tests for
//! `rustasea-image` (ADOPT-024).
//!
//! Storage and queued-transform tests live in `storage_job.rs`. Fixtures come
//! from the shared `common` module (all generated in memory).

mod common;

use common::{decode, gradient_png, jpeg_bytes, jpeg_with_orientation, solid_png};
use rustasea_image::{Anchor, Format, Image, ImageError, WatermarkOptions};

// --- Pipeline dimensions ---------------------------------------------------

/// `fit` scales a 4000×3000 source to the 800×600 box.
#[test]
fn pipeline_fit_dimensions() {
    let bytes = jpeg_bytes(4000, 3000);
    let dims = Image::from_bytes(bytes).fit(800, 600).dimensions().unwrap();
    assert_eq!(dims, (800, 600));
}

/// `cover` and `thumbnail` both yield the exact target box.
#[test]
fn pipeline_cover_and_thumbnail() {
    let bytes = jpeg_bytes(4000, 3000);
    assert_eq!(
        Image::from_bytes(bytes.clone())
            .cover(400, 400)
            .dimensions()
            .unwrap(),
        (400, 400)
    );
    assert_eq!(
        Image::from_bytes(bytes)
            .thumbnail(400, 400)
            .dimensions()
            .unwrap(),
        (400, 400)
    );
}

/// `resize` is exact and `crop` cuts the requested rectangle.
#[test]
fn pipeline_resize_and_crop() {
    let bytes = jpeg_bytes(400, 300);
    assert_eq!(
        Image::from_bytes(bytes.clone())
            .resize(123, 77)
            .dimensions()
            .unwrap(),
        (123, 77)
    );
    assert_eq!(
        Image::from_bytes(bytes)
            .crop(50, 40, 200, 100)
            .dimensions()
            .unwrap(),
        (200, 100)
    );
}

/// Chained ops compose in order (resize then cover).
#[test]
fn pipeline_chained_ops() {
    let bytes = jpeg_bytes(1000, 500);
    let dims = Image::from_bytes(bytes)
        .resize(500, 500)
        .cover(64, 64)
        .dimensions()
        .unwrap();
    assert_eq!(dims, (64, 64));
}

// --- Encode round-trips ----------------------------------------------------

/// A PNG re-encoded to JPEG decodes back at the same dimensions.
#[test]
fn round_trip_png_to_jpeg() {
    let bytes = gradient_png(120, 90);
    let jpeg = Image::from_bytes(bytes)
        .encode(Format::Jpeg)
        .to_bytes()
        .unwrap();
    assert_eq!(&jpeg[0..2], &[0xFF, 0xD8]);
    let back = decode(&jpeg);
    assert_eq!((back.width(), back.height()), (120, 90));
}

/// A lower JPEG quality yields fewer bytes on a gradient image.
#[test]
fn quality_affects_size() {
    let bytes = gradient_png(256, 256);
    let high = Image::from_bytes(bytes.clone())
        .encode(Format::Jpeg)
        .quality(100)
        .to_bytes()
        .unwrap();
    let low = Image::from_bytes(bytes)
        .encode(Format::Jpeg)
        .quality(10)
        .to_bytes()
        .unwrap();
    assert!(
        low.len() < high.len(),
        "low={} high={}",
        low.len(),
        high.len()
    );
}

/// Quality 0 and 101 are typed errors.
#[test]
fn invalid_quality_is_typed() {
    let bytes = gradient_png(10, 10);
    let err = Image::from_bytes(bytes.clone())
        .quality(0)
        .to_bytes()
        .unwrap_err();
    assert!(matches!(err, ImageError::InvalidQuality(0)));
    let err = Image::from_bytes(bytes)
        .quality(101)
        .to_bytes()
        .unwrap_err();
    assert!(matches!(err, ImageError::InvalidQuality(101)));
}

/// WebP round-trips through the lossless encoder.
#[test]
fn round_trip_webp() {
    let bytes = gradient_png(64, 48);
    let webp = Image::from_bytes(bytes)
        .encode(Format::Webp)
        .to_bytes()
        .unwrap();
    assert_eq!(&webp[0..4], b"RIFF");
    let back = decode(&webp);
    assert_eq!((back.width(), back.height()), (64, 48));
}

// --- EXIF orientation ------------------------------------------------------

/// Orientation 6 (rotate 90 CW) swaps the axes under auto-orient.
#[test]
fn exif_orientation_6_swaps_axes() {
    let raw = jpeg_with_orientation(120, 60, 6);
    // Without auto-orient the dims match the stored pixels.
    let plain = Image::from_bytes(raw.clone())
        .auto_orient(false)
        .dimensions()
        .unwrap();
    assert_eq!(plain, (120, 60));
    // With auto-orient (the default) orientation 6 rotates 90° CW → swapped.
    let oriented = Image::from_bytes(raw).dimensions().unwrap();
    assert_eq!(oriented, (60, 120));
}

/// Orientation 1 is a no-op even with auto-orient enabled.
#[test]
fn exif_orientation_1_is_identity() {
    let raw = jpeg_with_orientation(120, 60, 1);
    assert_eq!(Image::from_bytes(raw).dimensions().unwrap(), (120, 60));
}

/// A non-JPEG (PNG) source ignores the EXIF path without error.
#[test]
fn exif_ignored_for_non_jpeg() {
    let bytes = solid_png(40, 20, [10, 20, 30]);
    assert_eq!(Image::from_bytes(bytes).dimensions().unwrap(), (40, 20));
}

// --- Watermark -------------------------------------------------------------

/// Text drawn bottom-right changes the expected region and leaves the far
/// corner untouched.
#[test]
fn watermark_text_changes_region() {
    let bytes = solid_png(100, 50, [255, 255, 255]);
    let opts = WatermarkOptions {
        anchor: Anchor::BottomRight,
        x: 0,
        y: 0,
        scale: 1.0,
        color: [0, 0, 0],
        opacity: 1.0,
    };
    let out = Image::from_bytes(bytes)
        .watermark_text("AB", opts)
        .encode(Format::Png)
        .to_bytes()
        .unwrap();
    let img = decode(&out).to_rgba8();
    // The glyph block sits in the bottom-right corner.
    let mut changed = 0;
    for y in 40..50 {
        for x in 80..100 {
            if img.get_pixel(x, y).0[0] < 128 {
                changed += 1;
            }
        }
    }
    assert!(changed > 0, "expected dark watermark pixels");
    // The top-left corner is outside the watermark region.
    assert_eq!(img.get_pixel(2, 2).0, [255, 255, 255, 255]);
}

/// An image watermark blends a solid overlay into its region only.
#[test]
fn watermark_image_changes_region() {
    let base = solid_png(40, 40, [255, 255, 255]);
    let overlay = solid_png(10, 10, [0, 0, 0]);
    let out = Image::from_bytes(base)
        .watermark_image(overlay, 0, 0, 1.0)
        .encode(Format::Png)
        .to_bytes()
        .unwrap();
    let img = decode(&out).to_rgba8();
    assert_eq!(img.get_pixel(5, 5).0, [0, 0, 0, 255]);
    assert_eq!(img.get_pixel(30, 30).0, [255, 255, 255, 255]);
}

/// A fully-transparent watermark pixel leaves the destination unchanged.
#[test]
fn watermark_zero_opacity_is_noop() {
    let base = solid_png(20, 20, [255, 255, 255]);
    let overlay = solid_png(5, 5, [0, 0, 0]);
    let out = Image::from_bytes(base)
        .watermark_image(overlay, 0, 0, 0.0)
        .encode(Format::Png)
        .to_bytes()
        .unwrap();
    let img = decode(&out).to_rgba8();
    assert_eq!(img.get_pixel(2, 2).0, [255, 255, 255, 255]);
}

// --- Unsupported / malformed ----------------------------------------------

/// Garbage bytes surface a typed error, never a panic.
#[test]
fn garbage_bytes_typed_error() {
    let err = Image::from_bytes(b"not an image".to_vec())
        .to_bytes()
        .unwrap_err();
    assert!(matches!(
        err,
        ImageError::UnsupportedFormat(_) | ImageError::Decode(_)
    ));
}

/// An unknown extension (with undecodable content) is a typed
/// unsupported-format error, never a panic.
#[test]
fn unknown_extension_unsupported() {
    assert_eq!(Format::from_extension("a/b/c.xyz"), None);
    let path = common::temp_path("unknown-ext", "xyz");
    std::fs::write(&path, b"not an image at all").unwrap();
    let err = Image::load(&path).to_bytes().unwrap_err();
    assert!(
        matches!(
            err,
            ImageError::UnsupportedFormat(_) | ImageError::Decode(_)
        ),
        "unexpected error: {err:?}"
    );
    let _ = std::fs::remove_file(&path);
}
