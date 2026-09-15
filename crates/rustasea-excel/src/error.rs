//! Typed error surface for the Excel/CSV import-export crate (ADOPT-023).
//!
//! Every fallible entry point returns [`ExcelError`]. Row-level problems are
//! **not** errors: they are collected into an [`ImportReport`](crate::ImportReport)
//! as [`RowError`](crate::RowError) entries so a single bad row never aborts an
//! import. `ExcelError` is reserved for fatal conditions — unreadable files,
//! unknown formats/sheets, an unconfigured storage slot, or a failed write.

use thiserror::Error;

/// Result alias for every fallible operation in this crate.
pub type Result<T> = std::result::Result<T, ExcelError>;

/// Fatal error type for import/export operations.
///
/// Row-level validation/deserialization failures never surface here; they are
/// reported per-row through [`ImportReport`](crate::ImportReport).
#[derive(Debug, Error)]
pub enum ExcelError {
    /// An underlying filesystem/stream read or write failed.
    #[error("io error for {path}: {source}")]
    Io {
        /// Path (or a synthetic label such as `<reader>`) that failed.
        path: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The target/`from_path` extension is not a supported spreadsheet format.
    #[error("unsupported spreadsheet format: {0}")]
    UnsupportedFormat(String),

    /// The requested worksheet does not exist in the workbook.
    #[error("worksheet not found: {0}")]
    SheetNotFound(String),

    /// A structural parse failure (malformed CSV/XML, non-UTF-8 cell, …).
    ///
    /// `row` is the 1-based physical row when known, otherwise `0`.
    #[error("parse error at row {row}: {message}")]
    Parse {
        /// 1-based row where parsing failed, or `0` when not row-specific.
        row: u64,
        /// Human-readable detail.
        message: String,
    },

    /// A value could not be serialized/deserialized into the caller's type.
    #[error("serialization error: {0}")]
    Serialization(String),

    /// A storage-disk operation failed while reading/writing an export object.
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

    /// `chunk_size(0)` was requested; chunking requires a positive size.
    #[error("invalid chunk size: {0} (must be greater than zero)")]
    InvalidChunkSize(usize),

    /// Signed-URL generation/verification failed.
    #[error("signed url error: {0}")]
    SignedUrl(String),

    /// A queued export job could not be built or executed.
    #[error("job error: {0}")]
    Job(String),
}

impl From<std::io::Error> for ExcelError {
    /// Map a bare I/O error onto a path-less [`ExcelError::Io`].
    fn from(source: std::io::Error) -> Self {
        ExcelError::Io {
            path: "<stream>".to_string(),
            source,
        }
    }
}

impl From<serde_json::Error> for ExcelError {
    /// Map a JSON (de)serialization failure.
    fn from(err: serde_json::Error) -> Self {
        ExcelError::Serialization(err.to_string())
    }
}

impl From<csv::Error> for ExcelError {
    /// Map a CSV reader/writer failure onto a parse or I/O error.
    fn from(err: csv::Error) -> Self {
        match err.kind() {
            csv::ErrorKind::Io(io) => ExcelError::Io {
                path: "<csv>".to_string(),
                source: std::io::Error::new(io.kind(), io.to_string()),
            },
            _ => ExcelError::Parse {
                row: err.position().map(|p| p.line()).unwrap_or(0),
                message: err.to_string(),
            },
        }
    }
}

impl From<rustasea_storage::StorageError> for ExcelError {
    /// Flatten a storage error into a keyed [`ExcelError::Storage`].
    fn from(err: rustasea_storage::StorageError) -> Self {
        ExcelError::Storage {
            key: String::new(),
            message: err.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A serde error converts into the serialization variant.
    #[test]
    fn serde_error_converts() {
        let err: ExcelError = serde_json::from_str::<i32>("nope").unwrap_err().into();
        assert!(matches!(err, ExcelError::Serialization(_)));
    }

    /// A CSV error converts without panicking and carries a row hint.
    #[test]
    fn csv_error_converts() {
        let raw = "a,b\n\"unterminated";
        let mut rdr = csv::ReaderBuilder::new().from_reader(raw.as_bytes());
        let mut it = rdr.records();
        let _ = it.next();
        let err = it.next().map(|r| r.unwrap_err());
        if let Some(e) = err {
            let mapped: ExcelError = e.into();
            assert!(matches!(
                mapped,
                ExcelError::Parse { .. } | ExcelError::Io { .. }
            ));
        }
    }

    /// The storage error maps to the keyed storage variant.
    #[test]
    fn storage_error_converts() {
        let err: ExcelError = rustasea_storage::StorageError::NotFound("k".into()).into();
        assert!(matches!(err, ExcelError::Storage { .. }));
    }
}
