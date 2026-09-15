//! Export pipeline — [`Excel::export`](crate::Excel::export).
//!
//! [`ExportBuilder`] serializes an iterator of rows into CSV or XLSX and writes
//! them through one of three terminals: [`to_bytes`](ExportBuilder::to_bytes),
//! [`to_path`](ExportBuilder::to_path), or [`store`](ExportBuilder::store).
//!
//! # Memory behaviour
//!
//! * `to_path` with CSV **streams**: each row goes straight from the iterator
//!   into a file-backed [`csv::Writer`], so peak memory is bounded by a single
//!   row plus the writer's buffer, independent of the row count.
//! * `to_bytes` and `store` **buffer in memory**: `to_bytes` returns the encoded
//!   bytes, and `store` must hand a complete `&[u8]` to
//!   [`Storage::put`](rustasea_storage::Storage::put), which offers no
//!   incremental write API. Both materialize the serialized rows (and, for CSV,
//!   the full encoded buffer).
//! * XLSX is **always assembled in memory** (the format is a zip container) and
//!   then flushed in one write, for every terminal.

use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::error::{ExcelError, Result};
use crate::report::ExportReport;
use crate::value::{value_to_export_cell, ExportCell};

/// Default number of rows between progress callbacks.
const DEFAULT_CHUNK_SIZE: usize = 500;

/// Progress callback: `(done, total)`; exports always pass `total = None`.
type ProgressFn = Box<dyn FnMut(u64, Option<u64>)>;

/// Builder for a row-iterator export.
///
/// Create one with [`Excel::export`](crate::Excel::export), configure headers,
/// sheet, and format, then call a terminal method. `I` is any `IntoIterator` of
/// `R: Serialize` (e.g. `Vec<MyRow>` or a mapped iterator).
pub struct ExportBuilder<I, R> {
    /// Source rows.
    rows: Option<I>,
    /// Explicit header names; derived from the first row when `None`.
    headers: Option<Vec<String>>,
    /// Worksheet name (XLSX only).
    sheet: Option<String>,
    /// Explicit target format; inferred from the path extension otherwise.
    format: Option<crate::ExportFormat>,
    /// Rows between progress callbacks.
    chunk_size: usize,
    /// Progress sink invoked with `(done, total)`.
    on_progress: Option<ProgressFn>,
    /// Marker for the row type.
    _marker: std::marker::PhantomData<R>,
}

impl<I, R> ExportBuilder<I, R>
where
    I: IntoIterator<Item = R>,
    R: Serialize,
{
    /// Create a builder over `rows` (used by [`Excel::export`](crate::Excel::export)).
    pub(crate) fn new(rows: I) -> Self {
        Self {
            rows: Some(rows),
            headers: None,
            sheet: None,
            format: None,
            chunk_size: DEFAULT_CHUNK_SIZE,
            on_progress: None,
            _marker: std::marker::PhantomData,
        }
    }

    /// Set explicit header names (otherwise derived from the first row's keys).
    pub fn headers(mut self, headers: Vec<String>) -> Self {
        self.headers = Some(headers);
        self
    }

    /// Set the worksheet name (XLSX only).
    pub fn sheet(mut self, sheet: impl Into<String>) -> Self {
        self.sheet = Some(sheet.into());
        self
    }

    /// Force the output format (otherwise inferred from the path extension).
    pub fn format(mut self, format: crate::ExportFormat) -> Self {
        self.format = Some(format);
        self
    }

    /// Rows between progress callbacks (default 500; clamped to at least 1).
    pub fn chunk_size(mut self, chunk_size: usize) -> Self {
        self.chunk_size = chunk_size.max(1);
        self
    }

    /// Install a progress sink invoked with `(done, total)`.
    ///
    /// `total` is always `None` for exports (the row count is only known once
    /// the iterator is exhausted).
    pub fn on_progress(mut self, f: impl FnMut(u64, Option<u64>) + 'static) -> Self {
        self.on_progress = Some(Box::new(f));
        self
    }

    /// Serialize all rows to an in-memory buffer.
    ///
    /// Returns the encoded bytes and an [`ExportReport`] labelled `<bytes>`.
    pub fn to_bytes(mut self) -> Result<(Vec<u8>, ExportReport)> {
        let format = self.format.unwrap_or(crate::ExportFormat::Csv);
        let prepared = self.prepare()?;
        let bytes = encode(
            &prepared,
            format,
            self.sheet.as_deref(),
            self.chunk_size,
            &mut self.on_progress,
        )?;
        let report = ExportReport::new("<bytes>", prepared.len(), bytes.len() as u64);
        Ok((bytes, report))
    }

    /// Serialize all rows to a filesystem path.
    ///
    /// The format is inferred from the extension unless [`format`](Self::format)
    /// was set. CSV output is **streamed** directly from the row iterator into a
    /// file-backed writer (bounded memory); XLSX is assembled in memory and then
    /// written once.
    pub fn to_path(mut self, path: impl AsRef<Path>) -> Result<ExportReport> {
        let path = path.as_ref();
        let format = match self.format {
            Some(f) => f,
            None => crate::ExportFormat::from_path(path)?,
        };
        match format {
            crate::ExportFormat::Csv => self.stream_csv_to_path(path),
            crate::ExportFormat::Xlsx => {
                let prepared = self.prepare()?;
                let bytes = encode(
                    &prepared,
                    format,
                    self.sheet.as_deref(),
                    self.chunk_size,
                    &mut self.on_progress,
                )?;
                std::fs::write(path, &bytes).map_err(|source| ExcelError::Io {
                    path: path.display().to_string(),
                    source,
                })?;
                Ok(ExportReport::new(
                    path.display().to_string(),
                    prepared.len(),
                    bytes.len() as u64,
                ))
            }
        }
    }

    /// Stream CSV rows straight from the iterator into `path`.
    ///
    /// No `Vec<Value>` of rows and no full `Vec<u8>` are materialized: headers
    /// are derived from the first row (when not set explicitly) and each row is
    /// written as it is pulled, so peak memory is one row plus the writer
    /// buffer. The report's byte count is the final on-disk size.
    fn stream_csv_to_path(mut self, path: &Path) -> Result<ExportReport> {
        let source = self
            .rows
            .take()
            .ok_or_else(|| ExcelError::NotConfigured("export rows already consumed".to_string()))?;
        let file = std::fs::File::create(path).map_err(|source| ExcelError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let mut writer = csv::WriterBuilder::new().from_writer(file);

        let mut headers = self.headers.take();
        // Explicit headers are written up front so an empty iterator still
        // produces a header-only file (matching the buffered terminal).
        if let Some(h) = headers.as_ref() {
            if !h.is_empty() {
                writer.write_record(h)?;
            }
        }

        let mut rows_written: u64 = 0;
        for row in source.into_iter() {
            let value = serde_json::to_value(row).map_err(ExcelError::from)?;
            if headers.is_none() {
                let derived = derive_headers(&value)?;
                if !derived.is_empty() {
                    writer.write_record(&derived)?;
                }
                headers = Some(derived);
            }
            let headers_ref = headers.as_deref().unwrap_or(&[]);
            let cells = row_cells(&value, headers_ref)?;
            let fields: Vec<String> = cells
                .iter()
                .map(|c| c.to_csv_field().unwrap_or_default())
                .collect();
            writer.write_record(&fields)?;
            rows_written += 1;
            report_progress(&mut self.on_progress, rows_written, self.chunk_size);
        }

        let file = writer.into_inner().map_err(|e| ExcelError::Io {
            path: path.display().to_string(),
            source: e.into_error(),
        })?;
        let bytes = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(ExportReport::new(
            path.display().to_string(),
            rows_written,
            bytes,
        ))
    }

    /// Serialize all rows and store the result on the configured storage disk.
    ///
    /// Requires [`Excel::set_storage`](crate::Excel::set_storage); otherwise an
    /// [`ExcelError::NotConfigured`] is returned. This is the synchronous
    /// terminal: it blocks the calling thread while the write completes, but is
    /// safe to call from any context (inside or outside a Tokio runtime). Inside
    /// async code prefer [`store_async`](Self::store_async).
    pub fn store(mut self, key: &str) -> Result<ExportReport> {
        let format = match self.format {
            Some(f) => f,
            None => crate::ExportFormat::from_path(key)?,
        };
        let prepared = self.prepare()?;
        let bytes = encode(
            &prepared,
            format,
            self.sheet.as_deref(),
            self.chunk_size,
            &mut self.on_progress,
        )?;
        crate::storage::put_bytes(key, &bytes)?;
        Ok(ExportReport::new(key, prepared.len(), bytes.len() as u64))
    }

    /// Async counterpart to [`store`](Self::store).
    ///
    /// Serializes all rows and awaits the write on the configured storage disk
    /// directly (no runtime bridging and no blocking of the current worker).
    /// Requires [`Excel::set_storage`](crate::Excel::set_storage); otherwise an
    /// [`ExcelError::NotConfigured`] is returned.
    pub async fn store_async(mut self, key: &str) -> Result<ExportReport> {
        let format = match self.format {
            Some(f) => f,
            None => crate::ExportFormat::from_path(key)?,
        };
        let prepared = self.prepare()?;
        let bytes = encode(
            &prepared,
            format,
            self.sheet.as_deref(),
            self.chunk_size,
            &mut self.on_progress,
        )?;
        crate::storage::put_bytes_async(key, &bytes).await?;
        Ok(ExportReport::new(key, prepared.len(), bytes.len() as u64))
    }

    /// Materialize headers (derived from the first row if needed) and rows.
    fn prepare(&mut self) -> Result<PreparedRows> {
        let source = self
            .rows
            .take()
            .ok_or_else(|| ExcelError::NotConfigured("export rows already consumed".to_string()))?;
        let mut headers = self.headers.take();
        let mut rows = Vec::new();
        for row in source.into_iter() {
            let value = serde_json::to_value(row).map_err(ExcelError::from)?;
            if headers.is_none() {
                headers = Some(derive_headers(&value)?);
            }
            rows.push(value);
        }
        Ok(PreparedRows {
            headers: headers.unwrap_or_default(),
            rows,
        })
    }
}

/// Rows prepared for rendering: resolved headers + serialized JSON values.
pub(crate) struct PreparedRows {
    /// Column names in output order.
    pub(crate) headers: Vec<String>,
    /// Row values (each a JSON object when headers were derived).
    pub(crate) rows: Vec<Value>,
}

impl PreparedRows {
    /// Number of data rows.
    pub(crate) fn len(&self) -> u64 {
        self.rows.len() as u64
    }
}

/// Derive column names from a serialized row, which must be an object.
fn derive_headers(value: &Value) -> Result<Vec<String>> {
    match value {
        Value::Object(map) => Ok(map.keys().cloned().collect()),
        _ => Err(ExcelError::Serialization(
            "cannot derive headers: rows are not JSON objects; set `.headers(..)`".to_string(),
        )),
    }
}

/// Encode prepared rows into bytes for `format`.
pub(crate) fn encode(
    prepared: &PreparedRows,
    format: crate::ExportFormat,
    sheet: Option<&str>,
    chunk_size: usize,
    progress: &mut Option<ProgressFn>,
) -> Result<Vec<u8>> {
    match format {
        crate::ExportFormat::Csv => render_csv(prepared, chunk_size, progress),
        crate::ExportFormat::Xlsx => render_xlsx(prepared, sheet, chunk_size, progress),
    }
}

/// Extract the ordered cells for `value` using `headers`.
///
/// A non-object row is treated as a single-column value only when exactly one
/// header is present; otherwise it is a serialization error.
fn row_cells(value: &Value, headers: &[String]) -> Result<Vec<ExportCell>> {
    match value {
        Value::Object(map) => Ok(headers
            .iter()
            .map(|h| {
                map.get(h)
                    .map(value_to_export_cell)
                    .unwrap_or(ExportCell::Blank)
            })
            .collect()),
        other if headers.len() == 1 => Ok(vec![value_to_export_cell(other)]),
        _ => Err(ExcelError::Serialization(
            "row is not a JSON object and headers are not single-column".to_string(),
        )),
    }
}

/// Render `prepared` as CSV bytes.
fn render_csv(
    prepared: &PreparedRows,
    chunk_size: usize,
    progress: &mut Option<ProgressFn>,
) -> Result<Vec<u8>> {
    let mut csv_writer = csv::WriterBuilder::new().from_writer(Vec::new());
    if !prepared.headers.is_empty() {
        csv_writer.write_record(&prepared.headers)?;
    }
    for (idx, value) in prepared.rows.iter().enumerate() {
        let cells = row_cells(value, &prepared.headers)?;
        let fields: Vec<String> = cells
            .iter()
            .map(|c| c.to_csv_field().unwrap_or_default())
            .collect();
        csv_writer.write_record(&fields)?;
        report_progress(progress, (idx + 1) as u64, chunk_size);
    }
    csv_writer.into_inner().map_err(|e| ExcelError::Io {
        path: "<csv>".to_string(),
        source: e.into_error(),
    })
}

/// Render `prepared` as an XLSX workbook, returning the encoded bytes.
#[cfg(feature = "xlsx")]
fn render_xlsx(
    prepared: &PreparedRows,
    sheet: Option<&str>,
    chunk_size: usize,
    progress: &mut Option<ProgressFn>,
) -> Result<Vec<u8>> {
    let mut workbook = rust_xlsxwriter::Workbook::new();
    let worksheet = workbook.add_worksheet();
    if let Some(name) = sheet {
        worksheet
            .set_name(name)
            .map_err(|e| ExcelError::Serialization(e.to_string()))?;
    }
    for (col, header) in prepared.headers.iter().enumerate() {
        worksheet
            .write_string(0, col as u16, header)
            .map_err(|e| ExcelError::Serialization(e.to_string()))?;
    }
    for (idx, value) in prepared.rows.iter().enumerate() {
        let row = (idx + 1) as u32;
        let cells = row_cells(value, &prepared.headers)?;
        for (col, cell) in cells.iter().enumerate() {
            write_cell(worksheet, row, col as u16, cell)?;
        }
        report_progress(progress, (idx + 1) as u64, chunk_size);
    }
    workbook
        .save_to_buffer()
        .map_err(|e| ExcelError::Serialization(e.to_string()))
}

/// XLSX rendering stub when the `xlsx` feature is disabled.
#[cfg(not(feature = "xlsx"))]
fn render_xlsx(
    _prepared: &PreparedRows,
    _sheet: Option<&str>,
    _chunk_size: usize,
    _progress: &mut Option<ProgressFn>,
) -> Result<Vec<u8>> {
    Err(ExcelError::UnsupportedFormat(
        "xlsx support is disabled (enable the `xlsx` feature)".to_string(),
    ))
}

/// Write one [`ExportCell`] into the worksheet at `(row, col)`.
#[cfg(feature = "xlsx")]
fn write_cell(
    worksheet: &mut rust_xlsxwriter::Worksheet,
    row: u32,
    col: u16,
    cell: &ExportCell,
) -> Result<()> {
    let result = match cell {
        ExportCell::Text(s) => worksheet.write_string(row, col, s).map(|_| ()),
        ExportCell::Integer(i) => worksheet.write_number(row, col, *i as f64).map(|_| ()),
        ExportCell::Float(f) => worksheet.write_number(row, col, *f).map(|_| ()),
        ExportCell::Bool(b) => worksheet.write_boolean(row, col, *b).map(|_| ()),
        ExportCell::Blank => return Ok(()),
    };
    result.map_err(|e| ExcelError::Serialization(e.to_string()))
}

/// Fire the progress callback every `chunk_size` rows.
fn report_progress(progress: &mut Option<ProgressFn>, done: u64, chunk_size: usize) {
    if let Some(cb) = progress.as_mut() {
        if done.is_multiple_of(chunk_size as u64) {
            cb(done, None);
        }
    }
}

// Tests live in a sibling file so this module stays within the 500-line cap.
#[cfg(test)]
#[path = "export_tests.rs"]
mod tests;
