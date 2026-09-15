//! Applying pipeline ops to a decoded image.
//!
//! Kept separate from [`crate::pipeline`] so the builder (source/settings) and
//! the pixel operations stay within the file-size standard. Geometry is computed
//! by [`crate::ops`]; this module only drives the `image` crate's resize/crop/
//! rotate primitives and the watermark helpers.

use image::{DynamicImage, RgbaImage};

use crate::error::{ImageError, Result};
use crate::load;
use crate::ops;
use crate::pipeline::Op;
use crate::watermark;

/// Apply every op to `img` in order.
pub(crate) fn apply_ops(mut img: DynamicImage, ops: &[Op]) -> Result<DynamicImage> {
    for op in ops {
        img = apply_op(img, op)?;
    }
    Ok(img)
}

/// Apply a single op to `img`.
fn apply_op(img: DynamicImage, op: &Op) -> Result<DynamicImage> {
    use image::imageops::FilterType;
    let filter = FilterType::Lanczos3;
    Ok(match op {
        Op::Resize(w, h) => {
            validate_dims(*w, *h)?;
            img.resize_exact(*w, *h, filter)
        }
        Op::Fit(max_w, max_h) => {
            let (w, h) = ops::fit_dims(img.width(), img.height(), *max_w, *max_h);
            if w == 0 || h == 0 {
                return Err(ImageError::InvalidDimension(format!(
                    "fit box {max_w}x{max_h}"
                )));
            }
            img.resize_exact(w, h, filter)
        }
        Op::Cover(w, h) => {
            validate_dims(*w, *h)?;
            let (sw, sh) = ops::cover_dims(img.width(), img.height(), *w, *h);
            let scaled = img.resize_exact(sw, sh, filter);
            let (ox, oy) = ops::cover_offset(sw, sh, *w, *h);
            scaled.crop_imm(ox, oy, *w, *h)
        }
        Op::Crop(x, y, w, h) => {
            validate_dims(*w, *h)?;
            // `x + w` / `y + h` can overflow `u32` for untrusted parameters;
            // `is_none_or(..)` treats an overflow (or a `None`) as out-of-bounds
            // instead of panicking in debug (or wrapping into a bogus in-bounds
            // value).
            let x_oob = x.checked_add(*w).is_none_or(|end| end > img.width());
            let y_oob = y.checked_add(*h).is_none_or(|end| end > img.height());
            if x_oob || y_oob {
                return Err(ImageError::InvalidDimension(format!(
                    "crop {x},{y},{w},{h} exceeds {}x{}",
                    img.width(),
                    img.height()
                )));
            }
            img.crop_imm(*x, *y, *w, *h)
        }
        Op::Rotate(degrees) => rotate(img, *degrees),
        Op::WatermarkText { text, opts } => {
            let mut canvas = img.to_rgba8();
            watermark::draw_text(&mut canvas, text, opts);
            DynamicImage::ImageRgba8(canvas)
        }
        Op::WatermarkImage {
            bytes,
            x,
            y,
            opacity,
        } => {
            let overlay = load::decode(bytes)?.to_rgba8();
            let mut canvas = img.to_rgba8();
            watermark::draw_image(&mut canvas, &overlay, *x, *y, *opacity);
            DynamicImage::ImageRgba8(canvas)
        }
    })
}

/// Validate a non-zero dimension pair.
fn validate_dims(w: u32, h: u32) -> Result<()> {
    if w == 0 || h == 0 {
        return Err(ImageError::InvalidDimension(format!("{w}x{h}")));
    }
    Ok(())
}

/// Rotate `img` clockwise by `degrees`.
fn rotate(img: DynamicImage, degrees: f32) -> DynamicImage {
    let norm = degrees.rem_euclid(360.0);
    let near = |t: f32| (norm - t).abs() < 0.5;
    if near(0.0) || near(360.0) {
        return img;
    }
    if near(90.0) {
        return img.rotate90();
    }
    if near(180.0) {
        return img.rotate180();
    }
    if near(270.0) {
        return img.rotate270();
    }
    rotate_arbitrary(&img, norm)
}

/// Nearest-neighbour arbitrary rotation (no `imageproc` dependency).
fn rotate_arbitrary(img: &DynamicImage, degrees: f32) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    let (nw, nh) = ops::rotate_dims(w, h, degrees);
    let rad = degrees.to_radians();
    let (sin, cos) = (rad.sin(), rad.cos());
    let src = img.to_rgba8();
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let (ncx, ncy) = (nw as f32 / 2.0, nh as f32 / 2.0);
    let mut out = RgbaImage::new(nw, nh);
    for (x, y, px) in out.enumerate_pixels_mut() {
        let dx = x as f32 + 0.5 - ncx;
        let dy = y as f32 + 0.5 - ncy;
        // Inverse-map the destination pixel back into the source frame.
        let sx = cos * dx + sin * dy + cx;
        let sy = -sin * dx + cos * dy + cy;
        let sxi = sx.floor();
        let syi = sy.floor();
        if sxi >= 0.0 && syi >= 0.0 && (sxi as u32) < w && (syi as u32) < h {
            *px = *src.get_pixel(sxi as u32, syi as u32);
        }
    }
    DynamicImage::ImageRgba8(out)
}

#[cfg(test)]
mod tests {
    use crate::pipeline::{ImageBuilder, Source};
    use crate::testutil;
    use crate::{Format, ImageError};

    /// A builder over in-memory PNG bytes with a PNG hint.
    fn png_builder(w: u32, h: u32) -> ImageBuilder {
        ImageBuilder::new(
            Source::Bytes(testutil::gradient_png(w, h)),
            Some(Format::Png),
        )
    }

    /// `fit` yields the expected box dimensions end to end.
    #[test]
    fn fit_pipeline() {
        let dims = png_builder(400, 300).fit(200, 200).dimensions().unwrap();
        assert_eq!(dims, (200, 150));
    }

    /// `cover`/`thumbnail` yield the exact target box.
    #[test]
    fn cover_pipeline() {
        assert_eq!(
            png_builder(400, 300).cover(120, 120).dimensions().unwrap(),
            (120, 120)
        );
        assert_eq!(
            png_builder(400, 300)
                .thumbnail(90, 90)
                .dimensions()
                .unwrap(),
            (90, 90)
        );
    }

    /// `resize` is exact and `crop` cuts the requested rectangle.
    #[test]
    fn resize_and_crop() {
        assert_eq!(
            png_builder(400, 300).resize(50, 80).dimensions().unwrap(),
            (50, 80)
        );
        assert_eq!(
            png_builder(400, 300)
                .crop(10, 20, 100, 60)
                .dimensions()
                .unwrap(),
            (100, 60)
        );
    }

    /// Rotation swaps dimensions for a quarter turn.
    #[test]
    fn rotate_pipeline() {
        assert_eq!(
            png_builder(400, 300).rotate(90.0).dimensions().unwrap(),
            (300, 400)
        );
        assert_eq!(
            png_builder(100, 100).rotate(45.0).dimensions().unwrap(),
            (141, 141)
        );
    }

    /// A crop larger than the image is rejected.
    #[test]
    fn oversized_crop() {
        let err = png_builder(10, 10)
            .crop(5, 5, 20, 20)
            .to_bytes()
            .unwrap_err();
        assert!(matches!(err, ImageError::InvalidDimension(_)));
    }

    /// A crop whose `x + w` overflows `u32` yields a typed error, not a panic.
    #[test]
    fn crop_x_plus_w_overflow_is_typed_error() {
        let err = png_builder(100, 100)
            .crop(u32::MAX - 5, 0, 10, 10)
            .to_bytes()
            .unwrap_err();
        assert!(matches!(err, ImageError::InvalidDimension(_)));
    }

    /// A crop whose `y + h` overflows `u32` yields a typed error, not a panic.
    #[test]
    fn crop_y_plus_h_overflow_is_typed_error() {
        let err = png_builder(100, 100)
            .crop(0, u32::MAX - 5, 10, 10)
            .to_bytes()
            .unwrap_err();
        assert!(matches!(err, ImageError::InvalidDimension(_)));
    }

    /// A crop ending exactly on the far edge is accepted.
    #[test]
    fn crop_exact_boundary_is_ok() {
        let dims = png_builder(100, 100)
            .crop(90, 80, 10, 20)
            .dimensions()
            .unwrap();
        assert_eq!(dims, (10, 20));
    }

    /// A crop one pixel past the far edge is rejected.
    #[test]
    fn crop_beyond_boundary_is_typed_error() {
        let err = png_builder(100, 100)
            .crop(91, 0, 10, 10)
            .to_bytes()
            .unwrap_err();
        assert!(matches!(err, ImageError::InvalidDimension(_)));
    }
}
