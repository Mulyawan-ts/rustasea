//! The fluent transform pipeline — [`ImageBuilder`] and op accumulation.
//!
//! [`ImageBuilder`] accumulates a source (path or bytes), an ordered list of
//! [`Op`]s, and encode settings. Nothing is decoded until a terminal
//! ([`to_bytes`](ImageBuilder::to_bytes), [`to_path`](ImageBuilder::to_path),
//! [`store`](ImageBuilder::store), [`dimensions`](ImageBuilder::dimensions))
//! runs; at that point the pipeline decodes once, applies auto-orient, applies
//! each op in order (see [`crate::apply`]), and encodes. Geometry is computed by
//! [`crate::ops`] so the resize/fit/cover math is shared with its unit tests.

use std::path::{Path, PathBuf};

use image::DynamicImage;

use crate::error::{ImageError, Result};
use crate::format::Format;
use crate::watermark::WatermarkOptions;
use crate::{apply, encode, load};

/// Where the source pixels come from.
#[derive(Debug, Clone)]
pub(crate) enum Source {
    /// A file path, read lazily at terminal time.
    Path(PathBuf),
    /// An in-memory byte buffer.
    Bytes(Vec<u8>),
}

/// One transform in the pipeline, applied in insertion order.
#[derive(Debug, Clone)]
pub(crate) enum Op {
    /// Resize to exact dimensions (may distort).
    Resize(u32, u32),
    /// Fit inside a box, preserving aspect ratio.
    Fit(u32, u32),
    /// Cover a box, center-cropping the overflow.
    Cover(u32, u32),
    /// Crop a `x,y,w,h` rectangle.
    Crop(u32, u32, u32, u32),
    /// Rotate clockwise by `degrees`.
    Rotate(f32),
    /// Draw a text watermark.
    WatermarkText {
        /// Text to draw (ASCII subset).
        text: String,
        /// Placement and styling.
        opts: WatermarkOptions,
    },
    /// Draw an image watermark at `(x, y)`.
    WatermarkImage {
        /// Overlay image bytes.
        bytes: Vec<u8>,
        /// Left offset in base-image pixels.
        x: u32,
        /// Top offset in base-image pixels.
        y: u32,
        /// Blend opacity in `0.0..=1.0`.
        opacity: f32,
    },
}

/// A report describing an encoded/stored image.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImageReport {
    /// Destination path or storage key.
    pub path: String,
    /// Output container format.
    pub format: Format,
    /// Output width in pixels.
    pub width: u32,
    /// Output height in pixels.
    pub height: u32,
    /// Encoded byte length.
    pub bytes: u64,
}

/// Fluent builder for an image transform pipeline.
///
/// Obtain one from [`Image::load`](crate::Image::load) or
/// [`Image::from_bytes`](crate::Image::from_bytes), chain ops, then finish with
/// a terminal.
pub struct ImageBuilder {
    /// Source pixels (read lazily).
    pub(crate) source: Source,
    /// Explicit output format set by [`encode`](Self::encode).
    pub(crate) format: Option<Format>,
    /// Format inferred at construction (path extension / magic bytes).
    pub(crate) format_hint: Option<Format>,
    /// Ordered transforms.
    pub(crate) ops: Vec<Op>,
    /// Encode quality (1..=100).
    pub(crate) quality: u8,
    /// Whether to apply EXIF orientation on load.
    pub(crate) auto_orient: bool,
    /// Whether metadata stripping is requested (documentation-only; see below).
    pub(crate) strip_metadata: bool,
}

impl ImageBuilder {
    /// Create a builder for `source` with a format hint.
    pub(crate) fn new(source: Source, format_hint: Option<Format>) -> Self {
        Self {
            source,
            format: None,
            format_hint,
            ops: Vec::new(),
            quality: 90,
            auto_orient: true,
            strip_metadata: true,
        }
    }

    /// Resize to exact `w`×`h` pixels (may distort the aspect ratio).
    pub fn resize(mut self, w: u32, h: u32) -> Self {
        self.ops.push(Op::Resize(w, h));
        self
    }

    /// Scale to the largest size that fits inside `max_w`×`max_h`, preserving
    /// the aspect ratio.
    pub fn fit(mut self, max_w: u32, max_h: u32) -> Self {
        self.ops.push(Op::Fit(max_w, max_h));
        self
    }

    /// Scale and center-crop so the result is exactly `w`×`h` (fills the box).
    pub fn cover(mut self, w: u32, h: u32) -> Self {
        self.ops.push(Op::Cover(w, h));
        self
    }

    /// Thumbnail to `w`×`h`. Implemented as [`cover`](Self::cover) — a distinct
    /// name for `intervention/image` API parity.
    pub fn thumbnail(self, w: u32, h: u32) -> Self {
        self.cover(w, h)
    }

    /// Crop the `w`×`h` rectangle whose top-left corner is `(x, y)`.
    pub fn crop(mut self, x: u32, y: u32, w: u32, h: u32) -> Self {
        self.ops.push(Op::Crop(x, y, w, h));
        self
    }

    /// Rotate clockwise by `degrees`. `90`/`180`/`270` use exact fast paths;
    /// other angles use nearest-neighbour resampling.
    pub fn rotate(mut self, degrees: f32) -> Self {
        self.ops.push(Op::Rotate(degrees));
        self
    }

    /// Draw a text watermark (ASCII subset — see [`crate::font`]).
    pub fn watermark_text(mut self, text: impl Into<String>, opts: WatermarkOptions) -> Self {
        self.ops.push(Op::WatermarkText {
            text: text.into(),
            opts,
        });
        self
    }

    /// Draw an image watermark at `(x, y)` with `opacity`.
    pub fn watermark_image(mut self, bytes: Vec<u8>, x: u32, y: u32, opacity: f32) -> Self {
        self.ops.push(Op::WatermarkImage {
            bytes,
            x,
            y,
            opacity,
        });
        self
    }

    /// Enable/disable EXIF auto-orient (default enabled).
    pub fn auto_orient(mut self, enabled: bool) -> Self {
        self.auto_orient = enabled;
        self
    }

    /// Request metadata stripping (default enabled).
    ///
    /// Metadata is stripped on **every** re-encode because the `image` encoder
    /// writes no EXIF; this flag documents intent and is validated for the
    /// `false` case (metadata is still not preserved — see the crate docs).
    pub fn strip_metadata(mut self, enabled: bool) -> Self {
        self.strip_metadata = enabled;
        self
    }

    /// Set encode quality (`1..=100`). Out-of-range values are rejected with
    /// [`ImageError::InvalidQuality`] at terminal time.
    pub fn quality(mut self, q: u8) -> Self {
        self.quality = q;
        self
    }

    /// Force the output container format.
    pub fn encode(mut self, format: Format) -> Self {
        self.format = Some(format);
        self
    }

    /// Encode the pipeline result to bytes.
    ///
    /// # Errors
    ///
    /// [`ImageError::InvalidQuality`], [`ImageError::UnsupportedFormat`],
    /// [`ImageError::Io`], [`ImageError::Decode`], or [`ImageError::Encode`].
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let (img, format) = self.render()?;
        encode::encode(&img, format, self.quality)
    }

    /// Encode the pipeline result and write it to `path`.
    ///
    /// # Errors
    ///
    /// As [`to_bytes`](Self::to_bytes), plus [`ImageError::Io`] on a write
    /// failure.
    pub fn to_path(&self, path: impl AsRef<Path>) -> Result<ImageReport> {
        let path = path.as_ref();
        let (img, format) = self.render()?;
        let bytes = encode::encode(&img, format, self.quality)?;
        std::fs::write(path, &bytes).map_err(|source| ImageError::Io {
            path: path.display().to_string(),
            source,
        })?;
        Ok(ImageReport {
            path: path.display().to_string(),
            format,
            width: img.width(),
            height: img.height(),
            bytes: bytes.len() as u64,
        })
    }

    /// Encode the pipeline result and store it on the global disk at `key`
    /// (synchronous bridge).
    ///
    /// # Errors
    ///
    /// As [`to_bytes`](Self::to_bytes), plus [`ImageError::NotConfigured`] when
    /// no storage disk is installed.
    pub fn store(&self, key: &str) -> Result<ImageReport> {
        let (img, format) = self.render()?;
        let bytes = encode::encode(&img, format, self.quality)?;
        crate::storage::put_bytes(key, &bytes)?;
        Ok(ImageReport {
            path: key.to_string(),
            format,
            width: img.width(),
            height: img.height(),
            bytes: bytes.len() as u64,
        })
    }

    /// Async variant of [`store`](Self::store).
    ///
    /// # Errors
    ///
    /// As [`store`](Self::store).
    pub async fn store_async(&self, key: &str) -> Result<ImageReport> {
        let (img, format) = self.render()?;
        let bytes = encode::encode(&img, format, self.quality)?;
        crate::storage::put_bytes_async(key, &bytes).await?;
        Ok(ImageReport {
            path: key.to_string(),
            format,
            width: img.width(),
            height: img.height(),
            bytes: bytes.len() as u64,
        })
    }

    /// Dimensions of the pipeline result without encoding.
    ///
    /// # Errors
    ///
    /// As [`to_bytes`](Self::to_bytes) minus the encode step.
    pub fn dimensions(&self) -> Result<(u32, u32)> {
        let (img, _) = self.render()?;
        Ok(load::dims(&img))
    }

    /// Resolve the output format: explicit → hint → magic-byte guess.
    pub(crate) fn resolve_format(&self, bytes: &[u8]) -> Result<Format> {
        if let Some(fmt) = self.format {
            return Ok(fmt);
        }
        if let Some(fmt) = self.format_hint {
            return Ok(fmt);
        }
        load::guess_format(bytes).ok_or_else(|| {
            ImageError::UnsupportedFormat("could not infer format from bytes".to_string())
        })
    }

    /// Read the raw source bytes.
    fn read_source(&self) -> Result<Vec<u8>> {
        match &self.source {
            Source::Bytes(b) => Ok(b.clone()),
            Source::Path(p) => std::fs::read(p).map_err(|source| ImageError::Io {
                path: p.display().to_string(),
                source,
            }),
        }
    }

    /// Decode, auto-orient, apply ops, and resolve the output format.
    fn render(&self) -> Result<(DynamicImage, Format)> {
        if !(1..=100).contains(&self.quality) {
            return Err(ImageError::InvalidQuality(self.quality));
        }
        let bytes = self.read_source()?;
        let format = self.resolve_format(&bytes)?;
        let mut img = load::decode(&bytes)?;
        if self.auto_orient {
            let orientation = load::read_exif_orientation(&bytes);
            img = load::apply_orientation(img, orientation);
        }
        let img = apply::apply_ops(img, &self.ops)?;
        Ok((img, format))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

    /// Build a builder over in-memory PNG bytes with a PNG hint.
    fn png_builder(w: u32, h: u32) -> ImageBuilder {
        ImageBuilder::new(
            Source::Bytes(testutil::gradient_png(w, h)),
            Some(Format::Png),
        )
    }

    /// An out-of-range quality is a typed error.
    #[test]
    fn invalid_quality() {
        let err = png_builder(10, 10).quality(0).to_bytes().unwrap_err();
        assert!(matches!(err, ImageError::InvalidQuality(0)));
        let err = png_builder(10, 10).quality(101).to_bytes().unwrap_err();
        assert!(matches!(err, ImageError::InvalidQuality(101)));
    }

    /// Auto-orient defaults on and can be disabled.
    #[test]
    fn auto_orient_default() {
        assert!(png_builder(10, 10).auto_orient);
        assert!(!png_builder(10, 10).auto_orient(false).auto_orient);
    }

    /// The output format resolves explicit → hint → magic bytes.
    #[test]
    fn format_resolution() {
        assert_eq!(
            png_builder(10, 10).resolve_format(&[]).unwrap(),
            Format::Png
        );
        assert_eq!(
            png_builder(10, 10)
                .encode(Format::Jpeg)
                .resolve_format(&[])
                .unwrap(),
            Format::Jpeg
        );
    }
}
