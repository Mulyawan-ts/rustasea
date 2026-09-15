//! Streaming import pipeline — [`Excel::import`](crate::Excel::import).
//!
//! [`ImportBuilder`] reads a CSV/XLSX source row by row, maps each row onto a
//! header→value object, deserializes it into `T`, optionally validates it with a
//! [`Rules`] set, and accumulates accepted rows into fixed-size chunks handed to
//! an `on_chunk` callback. A single bad row is recorded as a [`RowError`] and
//! never aborts the run; only I/O and structural parse failures are fatal.

use std::marker::PhantomData;
use std::path::PathBuf;

use rustasea_validation::{ErrorBag, Rules};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::error::{ExcelError, Result};
use crate::reader::{csv, detect_format, ExportFormat, RawSheet, ReadSeek};
use crate::report::{ImportReport, RowError};

/// Default number of accepted rows buffered before `on_chunk` fires.
const DEFAULT_CHUNK_SIZE: usize = 500;

/// Chunk callback: receives each full (and final) accepted-row buffer.
type ChunkFn<T> = Box<dyn FnMut(&[T]) -> Result<()>>;

/// Progress callback: `(done, total)`; `total` is `None` for streaming CSV.
type ProgressFn = Box<dyn FnMut(u64, Option<u64>)>;

/// Where an import reads from.
enum Source {
    /// A filesystem path; the format is detected from its extension.
    Path(PathBuf),
    /// A caller-supplied reader with an explicit format.
    Reader(Box<dyn ReadSeek>, ExportFormat),
}

/// Outcome of processing a single physical data row.
enum RowOutcome<T> {
    /// The row deserialized (and validated) successfully.
    Accepted(T),
    /// The row was rejected; carries one or more row errors.
    Rejected(Vec<RowError>),
}

/// Builder for a typed, streaming import.
///
/// Create one with [`Excel::import`](crate::Excel::import), configure the source
/// and options, then call [`ImportBuilder::run`]. `T` is the row type; each row
/// is deserialized from a header-keyed JSON object.
pub struct ImportBuilder<T> {
    /// Input source (path or reader).
    source: Option<Source>,
    /// Whether the first row is a header row (default `true`).
    has_headers: bool,
    /// Explicit header names overriding the header row.
    columns: Option<Vec<String>>,
    /// Worksheet name for XLSX sources.
    sheet: Option<String>,
    /// Optional row-level validation rules.
    rules: Option<Rules>,
    /// Accepted rows buffered before `on_chunk` fires.
    chunk_size: usize,
    /// Chunk sink invoked with each full (and final) buffer.
    on_chunk: Option<ChunkFn<T>>,
    /// Progress sink invoked per row with `(done, total)`.
    on_progress: Option<ProgressFn>,
    /// Marker for the row type.
    _marker: PhantomData<T>,
}

impl<T> ImportBuilder<T>
where
    T: DeserializeOwned + Send + 'static,
{
    /// Create an empty builder (used by [`Excel::import`](crate::Excel::import)).
    pub(crate) fn new() -> Self {
        Self {
            source: None,
            has_headers: true,
            columns: None,
            sheet: None,
            rules: None,
            chunk_size: DEFAULT_CHUNK_SIZE,
            on_chunk: None,
            on_progress: None,
            _marker: PhantomData,
        }
    }

    /// Read from a filesystem path; the format is auto-detected by extension.
    pub fn from_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.source = Some(Source::Path(path.into()));
        self
    }

    /// Read from an in-memory/streaming source with an explicit format.
    pub fn from_reader(mut self, reader: Box<dyn ReadSeek>, format: ExportFormat) -> Self {
        self.source = Some(Source::Reader(reader, format));
        self
    }

    /// Whether the first physical row is a header row (default `true`).
    pub fn has_headers(mut self, has_headers: bool) -> Self {
        self.has_headers = has_headers;
        self
    }

    /// Override the column names (the header row is still consumed when
    /// [`has_headers`](ImportBuilder::has_headers) is `true`).
    pub fn columns(mut self, columns: Vec<String>) -> Self {
        self.columns = Some(columns);
        self
    }

    /// Select a worksheet by name (XLSX sources only).
    pub fn sheet(mut self, sheet: impl Into<String>) -> Self {
        self.sheet = Some(sheet.into());
        self
    }

    /// Validate each row's object against these [`Rules`] before accepting it.
    pub fn rules(mut self, rules: Rules) -> Self {
        self.rules = Some(rules);
        self
    }

    /// Accepted rows buffered before `on_chunk` fires (default 500; `0` is an
    /// [`ExcelError::InvalidChunkSize`] at [`run`](ImportBuilder::run)).
    pub fn chunk_size(mut self, chunk_size: usize) -> Self {
        self.chunk_size = chunk_size;
        self
    }

    /// Install a chunk sink invoked with each full buffer and once at the end.
    pub fn on_chunk(mut self, f: impl FnMut(&[T]) -> Result<()> + 'static) -> Self {
        self.on_chunk = Some(Box::new(f));
        self
    }

    /// Install a progress sink invoked per row with `(done, total)`.
    ///
    /// `total` is the known data-row count for XLSX, `None` for streaming CSV.
    pub fn on_progress(mut self, f: impl FnMut(u64, Option<u64>) + 'static) -> Self {
        self.on_progress = Some(Box::new(f));
        self
    }

    /// Execute the import, returning an [`ImportReport`].
    ///
    /// Row-level failures are collected; I/O and structural parse failures abort
    /// with [`ExcelError`].
    pub fn run(mut self) -> Result<ImportReport> {
        if self.chunk_size == 0 {
            return Err(ExcelError::InvalidChunkSize(0));
        }
        let sheet = self.open()?;
        let header_offset: u64 = if self.has_headers { 1 } else { 0 };
        let data_total = sheet.total.map(|t| t.saturating_sub(header_offset));

        let mut rows = sheet.rows;
        let header = self.resolve_header(&mut rows)?;

        let mut report = ImportReport::new();
        let mut buffer: Vec<T> = Vec::with_capacity(self.chunk_size);
        let mut data_index: u64 = 0;

        for row in rows {
            let cells = row?;
            data_index += 1;
            let physical_row = data_index + header_offset;

            match self.accept_row(&header, cells, physical_row) {
                RowOutcome::Accepted(value) => {
                    report.imported += 1;
                    buffer.push(value);
                    if buffer.len() >= self.chunk_size {
                        self.flush(&mut buffer)?;
                    }
                }
                RowOutcome::Rejected(errors) => {
                    report.failed += 1;
                    report.errors.extend(errors);
                }
            }
            report.total += 1;
            if let Some(progress) = self.on_progress.as_mut() {
                progress(data_index, data_total);
            }
        }
        self.flush(&mut buffer)?;
        Ok(report)
    }

    /// Open the configured source into a [`RawSheet`].
    fn open(&mut self) -> Result<RawSheet> {
        let source = self
            .source
            .take()
            .ok_or_else(|| ExcelError::NotConfigured("import source".to_string()))?;
        match source {
            Source::Path(path) => {
                let (format, delimiter) = detect_format(&path)?;
                let file = std::fs::File::open(&path).map_err(|source| ExcelError::Io {
                    path: path.display().to_string(),
                    source,
                })?;
                Self::build_sheet(Box::new(file), format, delimiter, self.sheet.as_deref())
            }
            Source::Reader(reader, format) => {
                Self::build_sheet(reader, format, b',', self.sheet.as_deref())
            }
        }
    }

    /// Build a [`RawSheet`] for a concrete format.
    fn build_sheet(
        reader: Box<dyn ReadSeek>,
        format: ExportFormat,
        delimiter: u8,
        sheet: Option<&str>,
    ) -> Result<RawSheet> {
        match format {
            ExportFormat::Csv => Ok(csv::read(reader, delimiter)),
            ExportFormat::Xlsx => {
                #[cfg(feature = "xlsx")]
                {
                    crate::reader::xlsx::read(reader as Box<dyn std::io::Read>, sheet)
                }
                #[cfg(not(feature = "xlsx"))]
                {
                    let _ = (reader, delimiter, sheet);
                    Err(ExcelError::UnsupportedFormat(
                        "xlsx support is disabled (enable the `xlsx` feature)".to_string(),
                    ))
                }
            }
        }
    }

    /// Determine column names from explicit columns, the header row, or indices.
    fn resolve_header(
        &mut self,
        rows: &mut Box<dyn Iterator<Item = Result<Vec<Value>>>>,
    ) -> Result<Vec<String>> {
        if let Some(columns) = &self.columns {
            if self.has_headers {
                let _ = rows.next();
            }
            return Ok(columns.clone());
        }
        if self.has_headers {
            let header_row = rows.next().transpose()?.ok_or_else(|| ExcelError::Parse {
                row: 1,
                message: "empty source: header row expected".to_string(),
            })?;
            return Ok(header_row
                .iter()
                .enumerate()
                .map(|(idx, cell)| cell_to_header(idx, cell))
                .collect());
        }
        Ok(Vec::new())
    }

    /// Map one row's cells onto `T`, returning a [`RowOutcome`].
    fn accept_row(&self, header: &[String], cells: Vec<Value>, physical_row: u64) -> RowOutcome<T> {
        let mut map = Map::new();
        for (idx, cell) in cells.into_iter().enumerate() {
            let name = match header.get(idx) {
                Some(name) => name.clone(),
                None => format!("column_{idx}"),
            };
            map.insert(name, cell);
        }
        let object = Value::Object(map);

        if let Some(rules) = &self.rules {
            if let Err(bag) = rules.validate(&object) {
                return RowOutcome::Rejected(validation_errors(physical_row, &bag));
            }
        }

        match serde_json::from_value::<T>(object) {
            Ok(value) => RowOutcome::Accepted(value),
            Err(err) => RowOutcome::Rejected(vec![deserialize_error(physical_row, &err)]),
        }
    }

    /// Invoke the chunk sink with the current buffer and clear it.
    fn flush(&mut self, buffer: &mut Vec<T>) -> Result<()> {
        if buffer.is_empty() {
            return Ok(());
        }
        if let Some(on_chunk) = self.on_chunk.as_mut() {
            on_chunk(buffer)?;
        }
        buffer.clear();
        Ok(())
    }
}

/// Convert a header cell into a column name.
///
/// A blank (empty or null) header cell falls back to a positional
/// `column_{idx}` name so multiple blank headers never collide on `""` (which
/// would drop all but the last column when rows are folded into a JSON object).
fn cell_to_header(idx: usize, cell: &Value) -> String {
    match cell {
        Value::String(s) if !s.is_empty() => s.clone(),
        Value::Null | Value::String(_) => format!("column_{idx}"),
        other => other.to_string(),
    }
}

/// Flatten a validation [`ErrorBag`] into field-scoped row errors.
fn validation_errors(physical_row: u64, bag: &ErrorBag) -> Vec<RowError> {
    bag.fields()
        .flat_map(|(field, errs)| {
            errs.iter().map(move |e| {
                RowError::field(
                    physical_row,
                    field.clone(),
                    e.code.clone(),
                    e.message.clone(),
                )
            })
        })
        .collect()
}

/// Build a row error from a serde deserialization failure.
fn deserialize_error(physical_row: u64, err: &serde_json::Error) -> RowError {
    let message = err.to_string();
    let (field, code) = parse_missing_field(&message);
    match field {
        Some(name) => RowError::field(physical_row, name, code, message),
        None => RowError::row(physical_row, code, message),
    }
}

/// Classify a serde error, extracting the missing field name when present.
fn parse_missing_field(message: &str) -> (Option<String>, String) {
    const PREFIX: &str = "missing field `";
    if let Some(rest) = message.strip_prefix(PREFIX) {
        if let Some(end) = rest.find('`') {
            return (Some(rest[..end].to_string()), "missing_field".to_string());
        }
    }
    (None, "type_mismatch".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::io::Cursor;

    /// A row struct used across the pipeline tests.
    #[derive(Debug, Deserialize, PartialEq)]
    struct Person {
        /// Name column.
        name: String,
        /// Age column.
        age: i64,
    }

    /// A well-formed CSV imports every row and reports success.
    #[test]
    fn imports_valid_rows() {
        let data = b"name,age\nada,36\ngrace,45\n";
        let report = crate::Excel::import::<Person>()
            .from_reader(Box::new(Cursor::new(data.to_vec())), ExportFormat::Csv)
            .run()
            .unwrap();
        assert_eq!(report.total, 2);
        assert_eq!(report.imported, 2);
        assert!(report.is_ok());
    }

    /// A non-deserializable cell becomes a row error without aborting.
    #[test]
    fn bad_type_is_row_error() {
        let data = b"name,age\nada,36\ngrace,notanumber\n";
        let report = crate::Excel::import::<Person>()
            .from_reader(Box::new(Cursor::new(data.to_vec())), ExportFormat::Csv)
            .run()
            .unwrap();
        assert_eq!(report.imported, 1);
        assert_eq!(report.failed, 1);
        assert_eq!(report.errors[0].row, 3);
    }

    /// Explicit columns override the header row.
    #[test]
    fn explicit_columns_override_header() {
        let data = b"a,b\nada,36\n";
        let report = crate::Excel::import::<Person>()
            .from_reader(Box::new(Cursor::new(data.to_vec())), ExportFormat::Csv)
            .columns(vec!["name".to_string(), "age".to_string()])
            .run()
            .unwrap();
        assert_eq!(report.imported, 1);
    }

    /// A zero chunk size is rejected before any read.
    #[test]
    fn zero_chunk_size_errors() {
        let data = b"name,age\nada,36\n";
        let err = crate::Excel::import::<Person>()
            .from_reader(Box::new(Cursor::new(data.to_vec())), ExportFormat::Csv)
            .chunk_size(0)
            .run()
            .unwrap_err();
        assert!(matches!(err, ExcelError::InvalidChunkSize(0)));
    }

    /// A BOM-prefixed CSV imports: the BOM is stripped from the first header.
    #[test]
    fn imports_bom_prefixed_csv() {
        let mut data = Vec::new();
        data.extend_from_slice("\u{feff}".as_bytes());
        data.extend_from_slice(b"name,age\nada,36\n");
        let report = crate::Excel::import::<Person>()
            .from_reader(Box::new(Cursor::new(data)), ExportFormat::Csv)
            .run()
            .unwrap();
        assert_eq!(report.imported, 1);
        assert!(report.is_ok());
    }

    /// Blank header cells get unique positional keys so no column is dropped.
    #[test]
    fn blank_headers_get_unique_keys() {
        // Two blank headers around a named one.
        let data = b",name,\nada,36,99\n";
        let report = crate::Excel::import::<serde_json::Value>()
            .from_reader(Box::new(Cursor::new(data.to_vec())), ExportFormat::Csv)
            .run()
            .unwrap();
        assert_eq!(report.imported, 1);

        // Re-run capturing the row object to assert the header keys.
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = seen.clone();
        let _ = crate::Excel::import::<serde_json::Value>()
            .from_reader(Box::new(Cursor::new(data.to_vec())), ExportFormat::Csv)
            .on_chunk(move |chunk| {
                sink.lock().unwrap().extend_from_slice(chunk);
                Ok(())
            })
            .run()
            .unwrap();
        let rows = std::sync::Arc::try_unwrap(seen)
            .unwrap()
            .into_inner()
            .unwrap();
        let object = rows[0].as_object().unwrap();
        assert_eq!(object.get("column_0"), Some(&serde_json::json!("ada")));
        assert_eq!(object.get("name"), Some(&serde_json::json!(36)));
        assert_eq!(object.get("column_2"), Some(&serde_json::json!(99)));
        assert_eq!(object.len(), 3);
    }
}
