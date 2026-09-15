//! Queued transforms — [`ImageTransformJob`] (feature `queue`).
//!
//! [`ImageTransformJob`] is a serializable job that reads a source object from
//! the process-wide storage disk, applies an ordered list of [`OpSpec`]s, and
//! writes the result back to the disk at `key`. Dispatching it through
//! `rustasea-queue` moves the decode/transform/encode work off the request
//! path; the destination key is deterministic, so the caller knows where the
//! result lands without a result channel.

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::format::Format;
use crate::pipeline::{ImageBuilder, Source};
use crate::watermark::WatermarkOptions;

/// A serializable description of one pipeline transform.
///
/// Mirrors [`crate::pipeline::Op`] but with owned, JSON-friendly payloads so a
/// job can cross the queue boundary. `WatermarkImage` carries raw overlay bytes
/// (serialized as a byte array).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum OpSpec {
    /// Resize to exact dimensions.
    Resize {
        /// Target width.
        w: u32,
        /// Target height.
        h: u32,
    },
    /// Fit inside a box, preserving aspect ratio.
    Fit {
        /// Box width.
        max_w: u32,
        /// Box height.
        max_h: u32,
    },
    /// Cover a box, center-cropping the overflow.
    Cover {
        /// Box width.
        w: u32,
        /// Box height.
        h: u32,
    },
    /// Thumbnail (identical to `cover`, distinct name for API parity).
    Thumbnail {
        /// Box width.
        w: u32,
        /// Box height.
        h: u32,
    },
    /// Crop a rectangle.
    Crop {
        /// Left offset.
        x: u32,
        /// Top offset.
        y: u32,
        /// Width.
        w: u32,
        /// Height.
        h: u32,
    },
    /// Rotate clockwise by `degrees`.
    Rotate {
        /// Clockwise rotation angle.
        degrees: f32,
    },
    /// Draw a text watermark.
    WatermarkText {
        /// Text to draw (ASCII subset).
        text: String,
        /// Placement and styling.
        options: WatermarkOptions,
    },
    /// Draw an image watermark.
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

/// A queued image transform: source object → ops → destination object.
///
/// Build it with [`ImageTransformJob::new`], append ops with the fluent helpers
/// (or set [`ops`](ImageTransformJob::ops) directly), then dispatch via
/// `rustasea-queue`. `handle` resolves the global storage slot (installed with
/// [`Image::set_storage`](crate::Image::set_storage)); a missing slot becomes a
/// `JobError::Exception`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageTransformJob {
    /// Destination object key on the storage disk.
    pub key: String,
    /// Source object key to read and transform.
    pub source_key: String,
    /// Ordered transforms to apply.
    pub ops: Vec<OpSpec>,
    /// Output container format.
    pub format: Format,
    /// Encode quality (1..=100).
    pub quality: u8,
}

impl ImageTransformJob {
    /// Create a job transforming `source_key` into `key`.
    ///
    /// Defaults to JPEG output at quality 90 with no ops.
    pub fn new(source_key: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            source_key: source_key.into(),
            ops: Vec::new(),
            format: Format::Jpeg,
            quality: 90,
        }
    }

    /// Append a resize op.
    pub fn resize(mut self, w: u32, h: u32) -> Self {
        self.ops.push(OpSpec::Resize { w, h });
        self
    }

    /// Append a fit op.
    pub fn fit(mut self, max_w: u32, max_h: u32) -> Self {
        self.ops.push(OpSpec::Fit { max_w, max_h });
        self
    }

    /// Append a cover op.
    pub fn cover(mut self, w: u32, h: u32) -> Self {
        self.ops.push(OpSpec::Cover { w, h });
        self
    }

    /// Append a thumbnail op.
    pub fn thumbnail(mut self, w: u32, h: u32) -> Self {
        self.ops.push(OpSpec::Thumbnail { w, h });
        self
    }

    /// Append a crop op.
    pub fn crop(mut self, x: u32, y: u32, w: u32, h: u32) -> Self {
        self.ops.push(OpSpec::Crop { x, y, w, h });
        self
    }

    /// Append a rotate op.
    pub fn rotate(mut self, degrees: f32) -> Self {
        self.ops.push(OpSpec::Rotate { degrees });
        self
    }

    /// Append a text watermark op.
    pub fn watermark_text(mut self, text: impl Into<String>, options: WatermarkOptions) -> Self {
        self.ops.push(OpSpec::WatermarkText {
            text: text.into(),
            options,
        });
        self
    }

    /// Set the output format.
    pub fn format(mut self, format: Format) -> Self {
        self.format = format;
        self
    }

    /// Set the encode quality.
    pub fn quality(mut self, quality: u8) -> Self {
        self.quality = quality;
        self
    }

    /// Apply the ops to a builder over in-memory `bytes`, returning the builder.
    ///
    /// Shared by `handle` and tests so the job and the direct pipeline use one
    /// op-application path.
    fn build(self, bytes: Vec<u8>) -> ImageBuilder {
        let mut builder = ImageBuilder::new(Source::Bytes(bytes), None)
            .encode(self.format)
            .quality(self.quality);
        for op in self.ops {
            builder = match op {
                OpSpec::Resize { w, h } => builder.resize(w, h),
                OpSpec::Fit { max_w, max_h } => builder.fit(max_w, max_h),
                OpSpec::Cover { w, h } => builder.cover(w, h),
                OpSpec::Thumbnail { w, h } => builder.thumbnail(w, h),
                OpSpec::Crop { x, y, w, h } => builder.crop(x, y, w, h),
                OpSpec::Rotate { degrees } => builder.rotate(degrees),
                OpSpec::WatermarkText { text, options } => builder.watermark_text(text, options),
                OpSpec::WatermarkImage {
                    bytes,
                    x,
                    y,
                    opacity,
                } => builder.watermark_image(bytes, x, y, opacity),
            };
        }
        builder
    }

    /// Run the transform against the installed disk (shared by the job body and
    /// tests that need the result without a queue).
    ///
    /// # Errors
    ///
    /// [`ImageError::NotConfigured`], [`ImageError::Storage`], or any
    /// decode/encode error.
    pub async fn execute(self) -> Result<crate::ImageReport> {
        let source = crate::storage::get_bytes_async(&self.source_key).await?;
        let key = self.key.clone();
        let builder = self.build(source);
        builder.store_async(&key).await
    }
}

#[cfg(feature = "queue")]
#[async_trait::async_trait]
impl rustasea_queue::Job for ImageTransformJob {
    /// Read the source object, apply the ops, and write the result to `key`.
    ///
    /// Returns `JobError::Exception` when the storage slot is unset or any step
    /// fails, so the worker records a permanent failure instead of a panic.
    async fn handle(self) -> std::result::Result<(), rustasea_queue::JobError> {
        self.execute()
            .await
            .map(|_| ())
            .map_err(|e| rustasea_queue::JobError::Exception(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A job serializes to JSON and round-trips (queue transport requirement).
    #[test]
    fn job_round_trips() {
        let job = ImageTransformJob::new("src/a.png", "out/a.webp")
            .resize(100, 100)
            .cover(50, 50)
            .format(Format::Webp)
            .quality(80);
        let encoded = serde_json::to_string(&job).unwrap();
        let decoded: ImageTransformJob = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.source_key, "src/a.png");
        assert_eq!(decoded.key, "out/a.webp");
        assert_eq!(decoded.ops.len(), 2);
        assert_eq!(decoded.quality, 80);
    }

    /// A watermark op spec round-trips through JSON.
    #[test]
    fn watermark_spec_round_trips() {
        let job =
            ImageTransformJob::new("a", "b").watermark_text("HI", WatermarkOptions::default());
        let encoded = serde_json::to_string(&job).unwrap();
        let decoded: ImageTransformJob = serde_json::from_str(&encoded).unwrap();
        assert!(matches!(decoded.ops[0], OpSpec::WatermarkText { .. }));
    }
}
