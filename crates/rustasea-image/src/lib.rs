//! RustaSea Image — image manipulation (ADOPT-024).
//!
//! Parity target: `intervention/image`. The crate provides a fluent transform
//! pipeline over the `image` codec set, EXIF auto-orientation, a storage
//! round-trip terminal, and a queued transform job:
//!
//! * [`Image::load`] / [`Image::from_bytes`] build an [`ImageBuilder`] that
//!   accumulates ops (`resize`/`fit`/`cover`/`thumbnail`/`crop`/`rotate`/
//!   `watermark_text`/`watermark_image`) and encodes on a terminal.
//! * Terminals: [`to_bytes`](ImageBuilder::to_bytes),
//!   [`to_path`](ImageBuilder::to_path), [`store`](ImageBuilder::store) /
//!   [`store_async`](ImageBuilder::store_async), and
//!   [`dimensions`](ImageBuilder::dimensions).
//! * [`ImageTransformJob`] moves a transform off the request path onto the
//!   queue (feature `queue`).
//!
//! # Formats
//!
//! [`Format`] covers jpeg/png/webp/gif/bmp/tiff. JPEG honours the builder's
//! `quality`; PNG maps it onto the zlib compression level. WebP encodes
//! losslessly (the `image` codec has no lossy encoder); GIF uses the first
//! frame only (animated transforms are out of scope).
//!
//! # EXIF
//!
//! When `auto_orient` is on (the default) the JPEG APP1 `Exif` orientation tag
//! is parsed by hand and applied on load. Re-encoding with the `image` crate
//! writes no EXIF, so metadata is stripped on every encode; `strip_metadata`
//! documents this intent (setting it to `false` does **not** preserve EXIF —
//! the pixels are the only thing carried across).
//!
//! # Storage slot
//!
//! [`Image::set_storage`] installs one process-wide disk (ADR-0007) used by the
//! `store` terminal and by [`ImageTransformJob`]. Tests reset it with
//! [`Image::clear_storage`].
//!
//! # Features
//!
//! * `queue` (default) — the [`ImageTransformJob`]. Disable default features
//!   for a pipeline-only build.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod error;
pub mod format;
pub mod ops;
pub mod pipeline;
pub mod watermark;

#[cfg(feature = "queue")]
pub mod job;

pub(crate) mod apply;
pub(crate) mod encode;
pub(crate) mod font;
pub(crate) mod load;
pub(crate) mod storage;
#[cfg(test)]
mod testutil;

use std::path::Path;
use std::sync::Arc;

pub use crate::error::{ImageError, Result};
pub use crate::format::Format;
pub use crate::pipeline::{ImageBuilder, ImageReport};
pub use crate::watermark::{Anchor, WatermarkOptions};

#[cfg(feature = "queue")]
pub use crate::job::{ImageTransformJob, OpSpec};

/// Entry point for the image-manipulation facade.
///
/// `Image` is a namespace for the two builder constructors and the global
/// storage slot — mirroring Laravel's static facade without a global singleton
/// (ADR-0007).
pub struct Image;

impl Image {
    /// Start a transform pipeline from an image file at `path`.
    ///
    /// The file is read (and its format inferred) when a terminal runs. A path
    /// with an unknown extension is only rejected at that point — the magic
    /// bytes decide the codec.
    pub fn load(path: impl AsRef<Path>) -> ImageBuilder {
        let path = path.as_ref();
        let hint = Format::from_extension(path);
        ImageBuilder::new(pipeline::Source::Path(path.to_path_buf()), hint)
    }

    /// Start a transform pipeline from an in-memory image.
    ///
    /// The format is inferred from the magic bytes when a terminal runs, unless
    /// overridden with [`ImageBuilder::encode`].
    pub fn from_bytes(bytes: Vec<u8>) -> ImageBuilder {
        ImageBuilder::new(pipeline::Source::Bytes(bytes), None)
    }

    /// Install the process-wide storage disk used by `store`/`ImageTransformJob`.
    ///
    /// The first install wins; subsequent calls are ignored. Returns `true`
    /// when this call installed the disk.
    pub fn set_storage(storage: Arc<dyn rustasea_storage::Storage>) -> bool {
        storage::set(storage)
    }

    /// Remove the installed storage disk (test helper).
    pub fn clear_storage() {
        storage::clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A builder over garbage bytes fails decode with a typed error.
    #[test]
    fn garbage_bytes_are_unsupported() {
        let err = Image::from_bytes(b"not an image".to_vec())
            .to_bytes()
            .unwrap_err();
        assert!(matches!(
            err,
            ImageError::UnsupportedFormat(_) | ImageError::Decode(_)
        ));
    }

    /// `Image::load` over a missing file surfaces an io error.
    #[test]
    fn missing_file_is_io_error() {
        let err = Image::load("/no/such/file.png").to_bytes().unwrap_err();
        assert!(matches!(err, ImageError::Io { .. }));
    }
}
