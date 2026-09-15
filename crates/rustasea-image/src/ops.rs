//! Pure geometry math for the transform pipeline.
//!
//! Every function here is deterministic integer/float arithmetic with **no**
//! dependency on the `image` crate or on decoded pixel data, so the resize/fit/
//! cover/crop/rotate dimensions can be unit-tested directly. The pipeline
//! (`pipeline.rs`) computes target geometry with these helpers before applying
//! the corresponding `image` operation, which keeps the "what size will this
//! be?" logic in one reviewable place.

/// Fit `src_w`×`src_h` inside the `max_w`×`max_h` box, preserving aspect ratio.
///
/// The result is the largest box that fits *inside* the bounds (neither
/// dimension exceeds its limit) with the source aspect ratio. Returns `(0, 0)`
/// for any degenerate (zero) input.
pub fn fit_dims(src_w: u32, src_h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if src_w == 0 || src_h == 0 || max_w == 0 || max_h == 0 {
        return (0, 0);
    }
    let scale = (max_w as f64 / src_w as f64).min(max_h as f64 / src_h as f64);
    let w = (src_w as f64 * scale).round().max(1.0) as u32;
    let h = (src_h as f64 * scale).round().max(1.0) as u32;
    (w, h)
}

/// Cover the `w`×`h` box: scale so **both** dimensions are `>=` the target.
///
/// Returns the scaled source dimensions (rounded up) from which a centered
/// `w`×`h` crop is taken — see [`cover_offset`]. Returns `(0, 0)` for a
/// degenerate input.
pub fn cover_dims(src_w: u32, src_h: u32, w: u32, h: u32) -> (u32, u32) {
    if src_w == 0 || src_h == 0 || w == 0 || h == 0 {
        return (0, 0);
    }
    let scale = (w as f64 / src_w as f64).max(h as f64 / src_h as f64);
    let sw = (src_w as f64 * scale).ceil().max(w as f64) as u32;
    let sh = (src_h as f64 * scale).ceil().max(h as f64) as u32;
    (sw, sh)
}

/// Centered crop origin for a `scaled_w`×`scaled_h` image cropped to `w`×`h`.
///
/// Uses integer division, so an odd overflow leaves the extra pixel on the
/// right/bottom edge.
pub fn cover_offset(scaled_w: u32, scaled_h: u32, w: u32, h: u32) -> (u32, u32) {
    (
        scaled_w.saturating_sub(w) / 2,
        scaled_h.saturating_sub(h) / 2,
    )
}

/// Dimensions of a `w`×`h` image rotated by `degrees` (clockwise).
///
/// Multiples of 90° swap (90/270) or keep (0/180/360) the dimensions exactly;
/// any other angle returns the axis-aligned bounding box of the rotated
/// rectangle. Returns `(0, 0)` for a degenerate input.
pub fn rotate_dims(w: u32, h: u32, degrees: f32) -> (u32, u32) {
    if w == 0 || h == 0 {
        return (0, 0);
    }
    let norm = degrees.rem_euclid(360.0);
    let near = |target: f32| (norm - target).abs() < 0.5;
    if near(90.0) || near(270.0) {
        return (h, w);
    }
    if near(0.0) || near(180.0) || near(360.0) {
        return (w, h);
    }
    let rad = norm.to_radians() as f64;
    let (sin, cos) = (rad.sin().abs(), rad.cos().abs());
    let nw = (w as f64 * cos + h as f64 * sin).round().max(1.0) as u32;
    let nh = (w as f64 * sin + h as f64 * cos).round().max(1.0) as u32;
    (nw, nh)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `fit_dims` shrinks a wide source to the width limit.
    #[test]
    fn fit_wide_source() {
        assert_eq!(fit_dims(4000, 1000, 800, 600), (800, 200));
    }

    /// `fit_dims` shrinks a tall source to the height limit.
    #[test]
    fn fit_tall_source() {
        assert_eq!(fit_dims(1000, 4000, 800, 600), (150, 600));
    }

    /// `fit_dims` on a square source is limited by the smaller scale factor.
    #[test]
    fn fit_square_source() {
        assert_eq!(fit_dims(1000, 1000, 800, 600), (600, 600));
    }

    /// `fit_dims` never upscales beyond the box, and leaves an inside source
    /// untouched in proportion (here it does scale up to fill the box).
    #[test]
    fn fit_exact_and_zero() {
        assert_eq!(fit_dims(4000, 3000, 800, 600), (800, 600));
        assert_eq!(fit_dims(0, 3000, 800, 600), (0, 0));
        assert_eq!(fit_dims(4000, 3000, 0, 600), (0, 0));
    }

    /// `cover_dims` scales so both dimensions reach the box, then the centered
    /// offset is computed from the overflow.
    #[test]
    fn cover_square_target() {
        let (sw, sh) = cover_dims(4000, 3000, 400, 400);
        assert_eq!((sw, sh), (534, 400));
        assert_eq!(cover_offset(sw, sh, 400, 400), (67, 0));
    }

    /// `cover_dims` on a tall source overflows vertically.
    #[test]
    fn cover_tall_source() {
        let (sw, sh) = cover_dims(1000, 4000, 400, 400);
        assert_eq!((sw, sh), (400, 1600));
        assert_eq!(cover_offset(sw, sh, 400, 400), (0, 600));
    }

    /// An odd remainder leaves the extra pixel on the right/bottom edge.
    #[test]
    fn cover_offset_odd_remainder() {
        assert_eq!(cover_offset(101, 51, 100, 50), (0, 0));
        assert_eq!(cover_offset(103, 53, 100, 50), (1, 1));
    }

    /// Quarter-turn rotations swap or keep dimensions exactly.
    #[test]
    fn rotate_quarter_turns() {
        assert_eq!(rotate_dims(4000, 3000, 90.0), (3000, 4000));
        assert_eq!(rotate_dims(4000, 3000, 180.0), (4000, 3000));
        assert_eq!(rotate_dims(4000, 3000, 270.0), (3000, 4000));
        assert_eq!(rotate_dims(4000, 3000, -90.0), (3000, 4000));
        assert_eq!(rotate_dims(4000, 3000, 0.0), (4000, 3000));
    }

    /// An arbitrary angle grows to the rotated bounding box.
    #[test]
    fn rotate_arbitrary() {
        // A 100×100 square rotated 45° has a bounding box of ~141×141.
        assert_eq!(rotate_dims(100, 100, 45.0), (141, 141));
        assert_eq!(rotate_dims(0, 100, 45.0), (0, 0));
    }
}
