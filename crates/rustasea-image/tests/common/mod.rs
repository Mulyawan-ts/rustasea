//! Shared helpers for the `rustasea-image` integration tests.
//!
//! Every fixture is generated in memory with the `image` crate — there are no
//! binary test assets. The process-wide storage slot is guarded by a static
//! mutex here so tests that touch it serialize across files.
//!
//! This module is compiled once per integration-test binary, so helpers used by
//! only one of them would otherwise be flagged as dead code.
#![allow(dead_code)]

use std::sync::Mutex;

use image::{DynamicImage, ImageFormat, Rgb, RgbImage};

/// Serializes every test that touches the process-wide storage slot.
pub static STORAGE_LOCK: Mutex<()> = Mutex::new(());

/// Acquire the storage-slot lock, tolerating a poisoned mutex.
pub fn lock_storage() -> std::sync::MutexGuard<'static, ()> {
    STORAGE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// A unique temp path for a test artifact (process-scoped).
pub fn temp_path(stem: &str, ext: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "rustasea-image-{stem}-{}.{ext}",
        std::process::id()
    ))
}

/// Encode a solid RGB image as JPEG bytes.
pub fn jpeg_bytes(w: u32, h: u32) -> Vec<u8> {
    let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, Rgb([128, 128, 128])));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Jpeg).expect("jpeg");
    out.into_inner()
}

/// Encode a position-varying RGB image as PNG bytes (real compression delta).
pub fn gradient_png(w: u32, h: u32) -> Vec<u8> {
    let img = RgbImage::from_fn(w, h, |x, y| {
        Rgb([
            (x % 256) as u8,
            (y % 256) as u8,
            ((x * 7 + y * 13) % 256) as u8,
        ])
    });
    let mut out = std::io::Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(img)
        .write_to(&mut out, ImageFormat::Png)
        .expect("png");
    out.into_inner()
}

/// Encode a solid colour PNG.
pub fn solid_png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    let img = RgbImage::from_pixel(w, h, Rgb(rgb));
    let mut out = std::io::Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(img)
        .write_to(&mut out, ImageFormat::Png)
        .expect("png");
    out.into_inner()
}

/// Decode bytes into a `DynamicImage` (asserting success).
pub fn decode(bytes: &[u8]) -> DynamicImage {
    image::load_from_memory(bytes).expect("decode")
}

/// A 26-byte TIFF block with a single IFD0 orientation entry.
pub fn tiff_orientation_block(orientation: u8, little: bool) -> Vec<u8> {
    let mut b = Vec::with_capacity(26);
    if little {
        b.extend_from_slice(b"II");
        b.extend_from_slice(&42u16.to_le_bytes());
        b.extend_from_slice(&8u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&0x0112u16.to_le_bytes());
        b.extend_from_slice(&3u16.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&[orientation, 0, 0, 0]);
        b.extend_from_slice(&0u32.to_le_bytes());
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

/// Splice a minimal Exif APP1 orientation segment into a JPEG.
pub fn jpeg_with_orientation(w: u32, h: u32, orientation: u8) -> Vec<u8> {
    let tiff = tiff_orientation_block(orientation, true);
    let payload_len = 6 + tiff.len();
    let seg_len = (payload_len + 2) as u16;
    let mut app1 = Vec::new();
    app1.extend_from_slice(&[0xFF, 0xE1]);
    app1.extend_from_slice(&seg_len.to_be_bytes());
    app1.extend_from_slice(b"Exif\0\0");
    app1.extend_from_slice(&tiff);
    let base = jpeg_bytes(w, h);
    let mut out = Vec::new();
    out.extend_from_slice(&base[0..2]);
    out.extend_from_slice(&app1);
    out.extend_from_slice(&base[2..]);
    out
}
