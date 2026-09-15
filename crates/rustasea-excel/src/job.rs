//! Queued export — [`ExportJob`] (feature `queue`).
//!
//! [`ExportJob`] is a serializable job carrying the fully-resolved rows and
//! headers of an export. Dispatching it through `rustasea-queue` moves the
//! potentially slow workbook build off the request path; its `handle` encodes
//! the bytes and writes them to the process-wide storage disk at the job's key.
//! The key is deterministic, so a caller knows exactly where the object will
//! land without any result channel.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{ExcelError, Result};
use crate::export::{encode, PreparedRows};
use crate::reader::ExportFormat;

/// A queued export job: rows + headers that encode to `key` on the storage disk.
///
/// Build it with [`ExportJob::new`], append rows with
/// [`ExportJob::row`], then dispatch via `rustasea-queue`. `handle` resolves the
/// global storage slot (installed with
/// [`Excel::set_storage`](crate::Excel::set_storage)) and writes the encoded
/// object; a missing slot becomes a `JobError::Exception`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportJob {
    /// Destination object key on the storage disk.
    pub key: String,
    /// Output container format.
    pub format: ExportFormat,
    /// Optional worksheet name (XLSX only).
    pub sheet: Option<String>,
    /// Column names in output order.
    pub headers: Vec<String>,
    /// Row payloads, each serialized to a JSON value.
    pub rows: Vec<Value>,
}

impl ExportJob {
    /// Create a job targeting `key` in `format` with `headers`.
    pub fn new(key: impl Into<String>, format: ExportFormat, headers: Vec<String>) -> Self {
        Self {
            key: key.into(),
            format,
            sheet: None,
            headers,
            rows: Vec::new(),
        }
    }

    /// Set the worksheet name (XLSX only).
    pub fn sheet(mut self, sheet: impl Into<String>) -> Self {
        self.sheet = Some(sheet.into());
        self
    }

    /// Append a row, serializing it to a JSON value.
    ///
    /// # Errors
    ///
    /// [`ExcelError::Serialization`] when `row` cannot be serialized.
    pub fn row(mut self, row: impl Serialize) -> Result<Self> {
        let value = serde_json::to_value(row).map_err(ExcelError::from)?;
        self.rows.push(value);
        Ok(self)
    }

    /// Number of rows queued so far.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }
}

#[cfg(feature = "queue")]
#[async_trait::async_trait]
impl rustasea_queue::Job for ExportJob {
    /// Encode the queued rows and write them to the global storage disk.
    ///
    /// Returns `JobError::Exception` when the storage slot is unset or the write
    /// fails, so the worker records a permanent failure instead of a panic.
    async fn handle(self) -> std::result::Result<(), rustasea_queue::JobError> {
        let prepared = PreparedRows {
            headers: self.headers,
            rows: self.rows,
        };
        let bytes = encode(
            &prepared,
            self.format,
            self.sheet.as_deref(),
            500,
            &mut None,
        )
        .map_err(|e| rustasea_queue::JobError::Exception(e.to_string()))?;
        crate::storage::put_bytes_async(&self.key, &bytes)
            .await
            .map_err(|e| rustasea_queue::JobError::Exception(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A job serializes to JSON and round-trips (queue transport requirement).
    #[test]
    fn job_round_trips() {
        let job = ExportJob::new("exports/a.csv", ExportFormat::Csv, vec!["n".into()])
            .row(json!({"n": 1}))
            .unwrap()
            .row(json!({"n": 2}))
            .unwrap();
        assert_eq!(job.row_count(), 2);
        let encoded = serde_json::to_string(&job).unwrap();
        let decoded: ExportJob = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.key, "exports/a.csv");
        assert_eq!(decoded.rows.len(), 2);
    }

    /// A serialization failure surfaces as a typed error.
    #[test]
    fn row_serialization_error() {
        struct Bad;
        impl Serialize for Bad {
            fn serialize<S: serde::Serializer>(
                &self,
                _s: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("nope"))
            }
        }
        let err = ExportJob::new("k.csv", ExportFormat::Csv, vec!["n".into()])
            .row(Bad)
            .unwrap_err();
        assert!(matches!(err, ExcelError::Serialization(_)));
    }
}
