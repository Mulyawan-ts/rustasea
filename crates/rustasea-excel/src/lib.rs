//! RustaSea Excel — Excel/CSV import-export (ADOPT-023).
//!
//! Parity target: `maatwebsite/excel`. The crate provides a streaming,
//! row-validated import pipeline and a chunked export pipeline over CSV and
//! XLSX, plus a queued export job and signed download links:
//!
//! * [`Excel::import`] builds a typed [`ImportBuilder`] that reads a CSV/XLSX
//!   source row by row, deserializes each row into `T`, validates it against a
//!   [`Rules`](rustasea_validation::Rules) set, and accumulates accepted rows
//!   into chunks. Bad rows become [`RowError`]s in an [`ImportReport`] and never
//!   abort the run.
//! * [`Excel::export`] builds an [`ExportBuilder`] that serializes any row
//!   iterator to CSV/XLSX and writes through [`to_bytes`](ExportBuilder::to_bytes),
//!   [`to_path`](ExportBuilder::to_path), or [`store`](ExportBuilder::store).
//! * [`ExportJob`] moves an export off the request path onto the queue (feature
//!   `queue`).
//! * [`ExportReport::signed_url`] mints a time-limited download link for an
//!   exported object (feature `signed-url`).
//!
//! # Features
//!
//! * `xlsx` (default) — XLSX read (`calamine`) and write (`rust_xlsxwriter`).
//! * `signed-url` (default) — [`ExportReport::signed_url`].
//! * `queue` (default) — the [`ExportJob`].
//!
//! CSV support is unconditional. Disable default features for a CSV-only build.
//!
//! # Storage slot
//!
//! [`Excel::set_storage`] installs one process-wide disk (ADR-0007) used by the
//! export `store` terminal and by [`ExportJob`]. Tests reset it with
//! [`Excel::clear_storage`].

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod error;
pub mod export;
pub mod import;
#[cfg(feature = "queue")]
pub mod job;
pub mod reader;
pub mod report;
pub(crate) mod storage;
pub mod value;

use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::Serialize;

pub use crate::error::{ExcelError, Result};
pub use crate::export::ExportBuilder;
pub use crate::import::ImportBuilder;
pub use crate::reader::{ExportFormat, ReadSeek};
pub use crate::report::{ExportReport, ImportReport, RowError};
pub use crate::value::{coerce_csv_cell, value_to_export_cell, ExportCell};

#[cfg(feature = "queue")]
pub use crate::job::ExportJob;

/// Entry point for the import/export facade.
///
/// `Excel` is a namespace for the two builders and the global storage slot —
/// mirroring Laravel's static facade without a global singleton (ADR-0007).
pub struct Excel;

impl Excel {
    /// Start building a typed import of `T`.
    ///
    /// Configure the source with [`from_path`](ImportBuilder::from_path) or
    /// [`from_reader`](ImportBuilder::from_reader), then call
    /// [`run`](ImportBuilder::run).
    pub fn import<T>() -> ImportBuilder<T>
    where
        T: DeserializeOwned + Send + 'static,
    {
        ImportBuilder::new()
    }

    /// Start building an export of `rows`.
    pub fn export<I, R>(rows: I) -> ExportBuilder<I, R>
    where
        I: IntoIterator<Item = R>,
        R: Serialize,
    {
        ExportBuilder::new(rows)
    }

    /// Install the process-wide storage disk used by `store`/`ExportJob`.
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

    /// `Excel::import` produces a builder that errors when no source is set.
    #[test]
    fn import_without_source_is_not_configured() {
        let err = Excel::import::<serde_json::Value>().run().unwrap_err();
        assert!(matches!(err, ExcelError::NotConfigured(_)));
    }

    /// `Excel::export` renders an empty iterator to an empty document.
    #[test]
    fn export_empty_iterator() {
        let rows: Vec<serde_json::Value> = Vec::new();
        let (bytes, report) = Excel::export(rows).to_bytes().unwrap();
        assert_eq!(report.rows, 0);
        assert!(bytes.is_empty());
    }
}
