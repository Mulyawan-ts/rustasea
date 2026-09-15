//! Source-format detection and the row-reading abstraction.
//!
//! [`ExportFormat`] names the two supported container formats. CSV is handled
//! unconditionally by the private `csv` submodule; XLSX support lives in the
//! private `xlsx` submodule behind the `xlsx` feature. Both produce a private
//! `RawSheet` — a stream of physical rows, each a `Vec<Value>` — so the import
//! pipeline is format-agnostic.

use std::io::{Read, Seek};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{ExcelError, Result};

pub(crate) mod csv;
#[cfg(feature = "xlsx")]
pub(crate) mod xlsx;

/// Object-safe reader bound used by [`ImportBuilder::from_reader`](crate::ImportBuilder::from_reader).
///
/// A blanket implementation covers every `Read + Seek` type, so callers pass a
/// `File`, `Cursor<Vec<u8>>`, or any custom source without extra glue.
pub trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

/// Supported spreadsheet container formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportFormat {
    /// Comma- (or tab-) separated values.
    Csv,
    /// Office Open XML workbook (`.xlsx` / `.xlsm`).
    Xlsx,
}

impl ExportFormat {
    /// Detect a format from a file extension (case-insensitive).
    ///
    /// `csv`/`tsv` map to [`ExportFormat::Csv`]; `xlsx`/`xlsm` map to
    /// [`ExportFormat::Xlsx`]. Anything else returns `None`.
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "csv" | "tsv" => Some(ExportFormat::Csv),
            "xlsx" | "xlsm" => Some(ExportFormat::Xlsx),
            _ => None,
        }
    }

    /// Detect a format from a path, or `UnsupportedFormat` on failure.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let (format, _) = detect_format(path.as_ref())?;
        Ok(format)
    }

    /// The canonical lowercase extension for this format.
    pub fn extension(&self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Xlsx => "xlsx",
        }
    }
}

impl std::fmt::Display for ExportFormat {
    /// Render the canonical extension.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.extension())
    }
}

/// Detect a format and CSV delimiter from a path extension.
///
/// Returns `UnsupportedFormat` when the extension is missing or unknown. The
/// delimiter is `\t` for `.tsv` and `,` otherwise.
pub(crate) fn detect_format(path: &Path) -> Result<(ExportFormat, u8)> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "csv" => Ok((ExportFormat::Csv, b',')),
        "tsv" => Ok((ExportFormat::Csv, b'\t')),
        "xlsx" | "xlsm" => Ok((ExportFormat::Xlsx, b',')),
        other => {
            let label = if other.is_empty() { "<none>" } else { other };
            Err(ExcelError::UnsupportedFormat(label.to_string()))
        }
    }
}

/// A format-agnostic stream of physical rows read from a source.
///
/// `rows` yields each physical row (header row included) as a `Vec<Value>`.
/// `total` is the known row count when the format can report it up front (XLSX
/// exposes the sheet height), or `None` for streaming CSV.
pub(crate) struct RawSheet {
    /// Physical rows in file order.
    pub rows: Box<dyn Iterator<Item = Result<Vec<Value>>>>,
    /// Total physical rows when known.
    pub total: Option<u64>,
}

impl RawSheet {
    /// Build a raw sheet from a materialized row vector (XLSX only).
    #[cfg(feature = "xlsx")]
    pub(crate) fn from_vec(rows: Vec<Vec<Value>>, total: Option<u64>) -> Self {
        Self {
            rows: Box::new(rows.into_iter().map(Ok)),
            total,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Extensions map case-insensitively onto formats.
    #[test]
    fn extension_detection_is_case_insensitive() {
        assert_eq!(ExportFormat::from_extension("CSV"), Some(ExportFormat::Csv));
        assert_eq!(
            ExportFormat::from_extension("Xlsx"),
            Some(ExportFormat::Xlsx)
        );
        assert_eq!(
            ExportFormat::from_extension("xlsm"),
            Some(ExportFormat::Xlsx)
        );
        assert_eq!(ExportFormat::from_extension("tsv"), Some(ExportFormat::Csv));
        assert_eq!(ExportFormat::from_extension("pdf"), None);
    }

    /// `detect_format` selects the tab delimiter for `.tsv`.
    #[test]
    fn detect_format_picks_delimiter() {
        let (fmt, delim) = detect_format(Path::new("data.tsv")).unwrap();
        assert_eq!(fmt, ExportFormat::Csv);
        assert_eq!(delim, b'\t');
        let err = detect_format(Path::new("data")).unwrap_err();
        assert!(matches!(err, ExcelError::UnsupportedFormat(_)));
    }

    /// Display renders the canonical extension.
    #[test]
    fn display_is_extension() {
        assert_eq!(ExportFormat::Csv.to_string(), "csv");
        assert_eq!(ExportFormat::Xlsx.to_string(), "xlsx");
    }
}
