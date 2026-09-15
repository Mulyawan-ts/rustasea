//! Typed error surface for the image-manipulation crate (ADOPT-024).
//!
//! Every fallible entry point returns [`ImageError`]. Decoding is
//! **best-effort**: an unknown extension or undecodable byte stream becomes a
//! typed [`ImageError::UnsupportedFormat`] / [`ImageError::Decode`] rather than
//! a panic. EXIF parsing never errors — a malformed APP1 block degrades to
//! "no orientation" so an otherwise valid image still loads.

use thiserror::Error;

/// Result alias for every fallible operation in this crate.
pub type Result<T> = std::result::Result<T, ImageError>;

/// Fatal error type for image load/transform/encode operations.
#[derive(Debug, Error)]
pub enum ImageError {
    /// An underlying filesystem read/write failed.
    #[error("io error for {path}: {source}")]
    Io {
        /// Path that failed.
        path: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The source bytes could not be decoded as an image.
    #[error("decode error: {0}")]
    Decode(String),

    /// The source extension/bytes are not a supported image format.
    #[error("unsupported image format: {0}")]
    UnsupportedFormat(String),

    /// The transformed image could not be encoded.
    #[error("encode error: {0}")]
    Encode(String),

    /// A storage-disk operation failed while reading/writing an object.
    #[error("storage error for {key}: {message}")]
    Storage {
        /// Object key that was being accessed.
        key: String,
        /// Flattened storage error detail.
        message: String,
    },

    /// A required global service (the storage slot) has not been configured.
    #[error("not configured: {0}")]
    NotConfigured(String),

    /// The requested JPEG/WebP quality is outside `1..=100`.
    #[error("invalid quality: {0} (must be between 1 and 100)")]
    InvalidQuality(u8),

    /// A dimension/geometry argument was invalid (e.g. a zero width).
    #[error("invalid dimension: {0}")]
    InvalidDimension(String),

    /// A queued transform job could not be built or executed.
    #[error("job error: {0}")]
    Job(String),
}

impl From<std::io::Error> for ImageError {
    /// Map a bare I/O error onto a path-less [`ImageError::Io`].
    fn from(source: std::io::Error) -> Self {
        ImageError::Io {
            path: "<stream>".to_string(),
            source,
        }
    }
}

impl From<rustasea_storage::StorageError> for ImageError {
    /// Flatten a storage error into a keyed [`ImageError::Storage`].
    fn from(err: rustasea_storage::StorageError) -> Self {
        ImageError::Storage {
            key: String::new(),
            message: err.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A storage error converts into the keyed storage variant.
    #[test]
    fn storage_error_converts() {
        let err: ImageError = rustasea_storage::StorageError::NotFound("k".into()).into();
        assert!(matches!(err, ImageError::Storage { .. }));
    }

    /// A bare I/O error converts into the path-less io variant.
    #[test]
    fn io_error_converts() {
        let err: ImageError = std::io::Error::other("boom").into();
        assert!(matches!(err, ImageError::Io { .. }));
    }
}
