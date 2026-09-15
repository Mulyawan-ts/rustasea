//! Watermark drawing — text (built-in bitmap font) and image overlays.
//!
//! Both paths blend onto an RGBA buffer with a shared alpha helper. Text uses
//! the [`crate::font`] 5×7 glyph table (ASCII subset); images are drawn at their
//! own size (callers pre-size the overlay) with a global opacity. Coordinates
//! are clamped to the image so an out-of-range watermark never panics.

use image::{Rgba, RgbaImage};

use crate::font::{glyph, GLYPH_H, GLYPH_W};

/// Where a watermark is anchored within the base image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    /// Top-left corner.
    TopLeft,
    /// Top-right corner.
    TopRight,
    /// Bottom-left corner.
    BottomLeft,
    /// Bottom-right corner (the default).
    #[default]
    BottomRight,
    /// Centered horizontally and vertically.
    Center,
}

/// Options for a text watermark.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WatermarkOptions {
    /// Reference corner the text is positioned from.
    pub anchor: Anchor,
    /// Horizontal offset (font pixels) from the anchor, before scaling.
    pub x: i32,
    /// Vertical offset (font pixels) from the anchor, before scaling.
    pub y: i32,
    /// Integer font scale factor (`1` = native 5×7 glyph).
    pub scale: f32,
    /// Text colour as RGB.
    pub color: [u8; 3],
    /// Blend opacity in `0.0..=1.0`.
    pub opacity: f32,
}

impl Default for WatermarkOptions {
    /// Bottom-right, 1× scale, black, fully opaque.
    fn default() -> Self {
        Self {
            anchor: Anchor::BottomRight,
            x: 0,
            y: 0,
            scale: 1.0,
            color: [0, 0, 0],
            opacity: 1.0,
        }
    }
}

/// Alpha-blend `src` (with colour `color`) onto `dst` at `opacity`.
///
/// `opacity` is clamped to `0.0..=1.0`; the destination alpha is untouched so
/// repeated blends accumulate colour without eroding coverage.
pub(crate) fn blend_pixel(dst: &mut Rgba<u8>, color: [u8; 3], opacity: f32) {
    let a = opacity.clamp(0.0, 1.0);
    for (d, s) in dst.0.iter_mut().zip(color.iter()) {
        let blended = *d as f32 * (1.0 - a) + *s as f32 * a;
        *d = blended.round().clamp(0.0, 255.0) as u8;
    }
}

/// Draw `text` onto `img` per `opts`. Returns the drawn bounding box
/// `(x, y, w, h)` in image pixels (empty when `text` has no glyphs).
///
/// The scale is bounded to `1.0..=100.0`; a non-finite scale (`NaN`, `±inf`)
/// falls back to `1.0`. This keeps the glyph loop finite for untrusted input.
pub(crate) fn draw_text(
    img: &mut RgbaImage,
    text: &str,
    opts: &WatermarkOptions,
) -> (u32, u32, u32, u32) {
    let scale = if opts.scale.is_finite() {
        opts.scale.clamp(1.0, 100.0)
    } else {
        1.0
    };
    let glyph_w = (GLYPH_W as f32 * scale).round() as u32;
    let glyph_h = (GLYPH_H as f32 * scale).round() as u32;
    let gap = (scale.round() as u32).max(1);
    let chars: Vec<char> = text.chars().collect();
    let text_w = if chars.is_empty() {
        0
    } else {
        chars.len() as u32 * glyph_w + (chars.len() as u32 - 1) * gap
    };
    let text_h = glyph_h;

    let (bx, by) = anchor_origin(img, opts.anchor, text_w, text_h, opts.x, opts.y);
    for (idx, ch) in chars.iter().enumerate() {
        let gx = bx + idx as i32 * (glyph_w + gap) as i32;
        draw_glyph(img, glyph(*ch), gx, by, scale, opts.color, opts.opacity);
    }
    (bx.max(0) as u32, by.max(0) as u32, text_w, text_h)
}

/// Draw one 5×7 glyph bitmap at image pixel `(bx, by)` scaled by `scale`.
fn draw_glyph(
    img: &mut RgbaImage,
    rows: [u8; 7],
    bx: i32,
    by: i32,
    scale: f32,
    color: [u8; 3],
    opacity: f32,
) {
    let s = scale.round().max(1.0) as i32;
    for (ry, row) in rows.iter().enumerate() {
        for cx in 0..GLYPH_W {
            let on = (row >> (GLYPH_W - 1 - cx)) & 1 == 1;
            if !on {
                continue;
            }
            for dy in 0..s {
                for dx in 0..s {
                    let px = bx + cx as i32 * s + dx;
                    let py = by + ry as i32 * s + dy;
                    if px < 0 || py < 0 || px >= img.width() as i32 || py >= img.height() as i32 {
                        continue;
                    }
                    blend_pixel(img.get_pixel_mut(px as u32, py as u32), color, opacity);
                }
            }
        }
    }
}

/// Compute the top-left draw origin for an anchored `w`×`h` block.
fn anchor_origin(
    img: &RgbaImage,
    anchor: Anchor,
    w: u32,
    h: u32,
    off_x: i32,
    off_y: i32,
) -> (i32, i32) {
    let iw = img.width() as i32;
    let ih = img.height() as i32;
    let (base_x, base_y) = match anchor {
        Anchor::TopLeft => (0, 0),
        Anchor::TopRight => (iw - w as i32, 0),
        Anchor::BottomLeft => (0, ih - h as i32),
        Anchor::BottomRight => (iw - w as i32, ih - h as i32),
        Anchor::Center => ((iw - w as i32) / 2, (ih - h as i32) / 2),
    };
    (base_x + off_x, base_y + off_y)
}

/// Alpha-blend `overlay` onto `img` at `(x, y)` with `opacity`.
///
/// The overlay is drawn at its own size; only the sub-rectangle that actually
/// intersects the canvas is visited, so a fully off-canvas overlay is a no-op
/// and the pixel offsets cannot overflow `u32` (both operands stay below the
/// corresponding canvas dimension).
pub(crate) fn draw_image(img: &mut RgbaImage, overlay: &RgbaImage, x: u32, y: u32, opacity: f32) {
    if x >= img.width() || y >= img.height() {
        return;
    }
    // Overlap extent, bounded by both the remaining canvas and the overlay.
    let w = (img.width() - x).min(overlay.width());
    let h = (img.height() - y).min(overlay.height());
    let alpha = opacity.clamp(0.0, 1.0);
    for oy in 0..h {
        for ox in 0..w {
            let src = overlay.get_pixel(ox, oy);
            let src_alpha = (src.0[3] as f32 / 255.0) * alpha;
            let color = [src.0[0], src.0[1], src.0[2]];
            blend_pixel(img.get_pixel_mut(x + ox, y + oy), color, src_alpha);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A full-opacity blend replaces the destination colour.
    #[test]
    fn blend_full_opacity() {
        let mut dst = Rgba([0, 0, 0, 255]);
        blend_pixel(&mut dst, [255, 255, 255], 1.0);
        assert_eq!(dst.0, [255, 255, 255, 255]);
    }

    /// A half-opacity blend averages the colours.
    #[test]
    fn blend_half_opacity() {
        let mut dst = Rgba([0, 0, 0, 255]);
        blend_pixel(&mut dst, [200, 100, 0], 0.5);
        assert_eq!(dst.0, [100, 50, 0, 255]);
    }

    /// The bottom-right anchor positions the block against the far corner.
    #[test]
    fn anchor_bottom_right() {
        let img = RgbaImage::new(100, 50);
        let (x, y) = anchor_origin(&img, Anchor::BottomRight, 10, 8, 0, 0);
        assert_eq!((x, y), (90, 42));
    }

    /// A solid image overlay changes the covered region only.
    #[test]
    fn image_overlay_region() {
        let mut base = RgbaImage::from_pixel(20, 20, Rgba([255, 255, 255, 255]));
        let overlay = RgbaImage::from_pixel(5, 5, Rgba([0, 0, 0, 255]));
        draw_image(&mut base, &overlay, 0, 0, 1.0);
        assert_eq!(base.get_pixel(2, 2).0, [0, 0, 0, 255]);
        assert_eq!(base.get_pixel(10, 10).0, [255, 255, 255, 255]);
    }

    /// A non-finite scale falls back to `1.0` and the draw completes (rather
    /// than looping for `i32::MAX` iterations).
    #[test]
    fn infinite_scale_completes() {
        let mut img = RgbaImage::from_pixel(50, 50, Rgba([0, 0, 0, 255]));
        let opts = WatermarkOptions {
            scale: f32::INFINITY,
            opacity: 1.0,
            color: [255, 255, 255],
            ..Default::default()
        };
        let (_, _, w, h) = draw_text(&mut img, "A", &opts);
        // Scale 1.0 → one 5×7 glyph.
        assert_eq!((w, h), (GLYPH_W, GLYPH_H));
    }

    /// A NaN scale also falls back to `1.0` without panicking.
    #[test]
    fn nan_scale_completes() {
        let mut img = RgbaImage::from_pixel(50, 50, Rgba([0, 0, 0, 255]));
        let opts = WatermarkOptions {
            scale: f32::NAN,
            ..Default::default()
        };
        let (_, _, w, h) = draw_text(&mut img, "A", &opts);
        assert_eq!((w, h), (GLYPH_W, GLYPH_H));
    }

    /// A fully off-canvas overlay is a no-op and does not panic.
    #[test]
    fn overlay_fully_outside_is_noop() {
        let mut base = RgbaImage::from_pixel(10, 10, Rgba([255, 255, 255, 255]));
        let overlay = RgbaImage::from_pixel(5, 5, Rgba([0, 0, 0, 255]));
        draw_image(&mut base, &overlay, 100, 100, 1.0);
        draw_image(&mut base, &overlay, u32::MAX, u32::MAX, 1.0);
        draw_image(&mut base, &overlay, 0, u32::MAX, 1.0);
        assert_eq!(base.get_pixel(0, 0).0, [255, 255, 255, 255]);
    }

    /// A partially overlapping overlay blends only the intersecting region.
    #[test]
    fn overlay_partial_overlap_clipped() {
        let mut base = RgbaImage::from_pixel(10, 10, Rgba([255, 255, 255, 255]));
        let overlay = RgbaImage::from_pixel(6, 6, Rgba([0, 0, 0, 255]));
        draw_image(&mut base, &overlay, 7, 7, 1.0);
        // Intersection is x,y ∈ 7..10.
        assert_eq!(base.get_pixel(7, 7).0, [0, 0, 0, 255]);
        assert_eq!(base.get_pixel(9, 9).0, [0, 0, 0, 255]);
        // Just outside the overlay's clipped extent stays untouched.
        assert_eq!(base.get_pixel(6, 6).0, [255, 255, 255, 255]);
        assert_eq!(base.get_pixel(9, 6).0, [255, 255, 255, 255]);
    }

    /// An `x` at `u32::MAX` is rejected by the early return without overflow.
    #[test]
    fn overlay_x_max_no_panic() {
        let mut base = RgbaImage::from_pixel(10, 10, Rgba([255, 255, 255, 255]));
        let overlay = RgbaImage::from_pixel(3, 3, Rgba([0, 0, 0, 255]));
        draw_image(&mut base, &overlay, u32::MAX, 0, 1.0);
        assert_eq!(base.get_pixel(0, 0).0, [255, 255, 255, 255]);
    }
}
