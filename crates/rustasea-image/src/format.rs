//! Output container formats and their mapping onto the `image` codec set.
//!
//! [`Format`] is the crate's stable, serde-friendly format token. It maps to
//! `image::ImageFormat` for encode/decode and offers extension/MIME inference
//! so a source path or uploaded content type selects the default output.

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Raster container formats supported by the transform pipeline.
///
/// The set mirrors the codecs enabled on the `image` dependency
/// (jpeg/png/webp/gif/bmp/tiff).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// JPEG (lossy; honours [`quality`](crate::ImageBuilder::quality)).
    Jpeg,
    /// PNG (lossless; `quality` maps to the zlib compression level).
    Png,
    /// WebP (lossless encode through the `image` codec).
    Webp,
    /// GIF (first frame only; animated transforms are out of scope).
    Gif,
    /// Windows BMP.
    Bmp,
    /// TIFF.
    Tiff,
}

impl Format {
    /// Infer a format from a path's file extension (case-insensitive).
    ///
    /// `jpg`/`jpeg` both map to [`Format::Jpeg`]. Returns `None` for an
    /// unknown or missing extension so the caller can raise
    /// [`ImageError::UnsupportedFormat`](crate::ImageError::UnsupportedFormat).
    pub fn from_extension(path: impl AsRef<Path>) -> Option<Format> {
        let ext = path.as_ref().extension()?.to_str()?.to_ascii_lowercase();
        Self::from_ext_str(&ext)
    }

    /// Infer a format from a bare extension string (no leading dot).
    pub fn from_ext_str(ext: &str) -> Option<Format> {
        match ext.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" => Some(Format::Jpeg),
            "png" => Some(Format::Png),
            "webp" => Some(Format::Webp),
            "gif" => Some(Format::Gif),
            "bmp" => Some(Format::Bmp),
            "tif" | "tiff" => Some(Format::Tiff),
            _ => None,
        }
    }

    /// Infer a format from a MIME content type (parameters ignored).
    pub fn from_mime(mime: &str) -> Option<Format> {
        let mime = mime.split(';').next()?.trim().to_ascii_lowercase();
        match mime.as_str() {
            "image/jpeg" | "image/jpg" => Some(Format::Jpeg),
            "image/png" => Some(Format::Png),
            "image/webp" => Some(Format::Webp),
            "image/gif" => Some(Format::Gif),
            "image/bmp" | "image/x-ms-bmp" => Some(Format::Bmp),
            "image/tiff" => Some(Format::Tiff),
            _ => None,
        }
    }

    /// The canonical lowercase file extension (no leading dot).
    pub fn to_extension(self) -> &'static str {
        match self {
            Format::Jpeg => "jpg",
            Format::Png => "png",
            Format::Webp => "webp",
            Format::Gif => "gif",
            Format::Bmp => "bmp",
            Format::Tiff => "tiff",
        }
    }

    /// The canonical MIME content type.
    pub fn to_mime(self) -> &'static str {
        match self {
            Format::Jpeg => "image/jpeg",
            Format::Png => "image/png",
            Format::Webp => "image/webp",
            Format::Gif => "image/gif",
            Format::Bmp => "image/bmp",
            Format::Tiff => "image/tiff",
        }
    }

    /// Map onto the `image` crate's format token.
    pub(crate) fn to_image_format(self) -> image::ImageFormat {
        match self {
            Format::Jpeg => image::ImageFormat::Jpeg,
            Format::Png => image::ImageFormat::Png,
            Format::Webp => image::ImageFormat::WebP,
            Format::Gif => image::ImageFormat::Gif,
            Format::Bmp => image::ImageFormat::Bmp,
            Format::Tiff => image::ImageFormat::Tiff,
        }
    }

    /// Map an `image` crate format token onto a [`Format`].
    ///
    /// Returns `None` for formats the crate does not advertise (e.g. avif).
    pub(crate) fn from_image_format(fmt: image::ImageFormat) -> Option<Format> {
        match fmt {
            image::ImageFormat::Jpeg => Some(Format::Jpeg),
            image::ImageFormat::Png => Some(Format::Png),
            image::ImageFormat::WebP => Some(Format::Webp),
            image::ImageFormat::Gif => Some(Format::Gif),
            image::ImageFormat::Bmp => Some(Format::Bmp),
            image::ImageFormat::Tiff => Some(Format::Tiff),
            _ => None,
        }
    }
}

impl fmt::Display for Format {
    /// Render the canonical lowercase name (`jpeg`, `png`, …).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Format::Jpeg => "jpeg",
            Format::Png => "png",
            Format::Webp => "webp",
            Format::Gif => "gif",
            Format::Bmp => "bmp",
            Format::Tiff => "tiff",
        };
        f.write_str(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Extensions infer the matching format, case-insensitively.
    #[test]
    fn extension_inference() {
        assert_eq!(Format::from_ext_str("JPG"), Some(Format::Jpeg));
        assert_eq!(Format::from_ext_str("jpeg"), Some(Format::Jpeg));
        assert_eq!(Format::from_ext_str("png"), Some(Format::Png));
        assert_eq!(Format::from_ext_str("webp"), Some(Format::Webp));
        assert_eq!(Format::from_ext_str("gif"), Some(Format::Gif));
        assert_eq!(Format::from_ext_str("bmp"), Some(Format::Bmp));
        assert_eq!(Format::from_ext_str("tiff"), Some(Format::Tiff));
        assert_eq!(Format::from_ext_str("tif"), Some(Format::Tiff));
        assert_eq!(Format::from_ext_str("xyz"), None);
    }

    /// Path extensions (with leading dot) infer the format.
    #[test]
    fn path_inference() {
        assert_eq!(Format::from_extension("a/b/c.PNG"), Some(Format::Png));
        assert_eq!(Format::from_extension("noext"), None);
    }

    /// MIME types infer the format; parameters are ignored.
    #[test]
    fn mime_inference() {
        assert_eq!(Format::from_mime("image/jpeg"), Some(Format::Jpeg));
        assert_eq!(
            Format::from_mime("image/png; charset=binary"),
            Some(Format::Png)
        );
        assert_eq!(Format::from_mime("text/plain"), None);
    }

    /// Extension and MIME round-trip through the canonical tokens.
    #[test]
    fn round_trip_tokens() {
        for fmt in [
            Format::Jpeg,
            Format::Png,
            Format::Webp,
            Format::Gif,
            Format::Bmp,
            Format::Tiff,
        ] {
            assert_eq!(Format::from_ext_str(fmt.to_extension()), Some(fmt));
            assert_eq!(Format::from_mime(fmt.to_mime()), Some(fmt));
            assert_eq!(Format::from_image_format(fmt.to_image_format()), Some(fmt));
        }
    }

    /// Display renders the canonical lowercase name.
    #[test]
    fn display_name() {
        assert_eq!(Format::Jpeg.to_string(), "jpeg");
        assert_eq!(Format::Webp.to_string(), "webp");
    }
}
