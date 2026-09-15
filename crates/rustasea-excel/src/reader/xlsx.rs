//! XLSX/XLSM reader built on `calamine` (feature `xlsx`).
//!
//! The whole workbook is buffered into memory because xlsx is a random-access
//! container (a zip of XML parts); parsing requires `Read + Seek + Clone`. Each
//! worksheet is then exposed as a [`RawSheet`] whose total row count is known
//! from the range height, enabling determinate progress reporting. To bound that
//! buffering, the reader refuses inputs larger than
//! [`DEFAULT_MAX_XLSX_BYTES`] (64 MiB) with a typed parse error.

use std::io::Read;

use calamine::{open_workbook_auto_from_rs, Data, Reader, Sheets};
use serde_json::{Number, Value};

use crate::error::{ExcelError, Result};
use crate::reader::RawSheet;

/// Maximum number of bytes read from an XLSX source before it is rejected.
///
/// The workbook is buffered whole (see the module docs), so this caps peak
/// memory for the read. 64 MiB comfortably fits real spreadsheets while
/// rejecting a hostile or runaway stream before it can exhaust memory.
pub const DEFAULT_MAX_XLSX_BYTES: u64 = 64 * 1024 * 1024;

/// Map a typed `calamine` cell onto a JSON value.
///
/// Empty and error cells become null. Dates map to their numeric Excel serial
/// (via `as_f64`) so no `chrono` feature is required — callers that need a
/// formatted date convert the serial themselves. XLSX stores every number as a
/// double, so an integral float (e.g. `36.0`) is emitted as an integer: this
/// keeps integer columns round-tripping through an export/import cycle while a
/// genuinely fractional value (`9.5`) stays a float. String cells that are empty
/// become null for symmetry with CSV's empty-field rule.
fn data_to_value(cell: &Data) -> Value {
    match cell {
        Data::Int(i) => Value::Number(Number::from(*i)),
        Data::Float(f) => float_to_value(*f),
        Data::Bool(b) => Value::Bool(*b),
        Data::String(s) => {
            if s.is_empty() {
                Value::Null
            } else {
                Value::String(s.clone())
            }
        }
        Data::DateTime(dt) => float_to_value(dt.as_f64()),
        Data::DateTimeIso(s) | Data::DurationIso(s) => Value::String(s.clone()),
        Data::Empty | Data::Error(_) => Value::Null,
    }
}

/// Convert a numeric cell to the narrowest faithful JSON number.
fn float_to_value(f: f64) -> Value {
    if !f.is_finite() {
        return Value::Null;
    }
    // An integral value within `i64` range round-trips as an integer so typed
    // integer columns survive an XLSX export/import cycle (xlsx has no int type).
    if f.fract() == 0.0 && f >= i64::MIN as f64 && f <= i64::MAX as f64 {
        return Value::Number(Number::from(f as i64));
    }
    Number::from_f64(f)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

/// Open a workbook from any `Read` source, buffering it into memory.
///
/// Reads at most [`DEFAULT_MAX_XLSX_BYTES`] + 1 bytes; anything longer is
/// rejected with a typed parse error instead of being buffered whole.
fn open(reader: Box<dyn Read>) -> Result<Sheets<std::io::Cursor<Vec<u8>>>> {
    open_with_limit(reader, DEFAULT_MAX_XLSX_BYTES)
}

/// Open a workbook, rejecting sources longer than `limit` bytes.
///
/// The limit is read through [`Read::take`] with one extra byte so an exactly
/// `limit`-sized input still succeeds while an over-limit one is detected
/// without buffering the whole stream.
fn open_with_limit(reader: Box<dyn Read>, limit: u64) -> Result<Sheets<std::io::Cursor<Vec<u8>>>> {
    let mut bytes = Vec::new();
    let mut reader = reader.take(limit.saturating_add(1));
    reader
        .read_to_end(&mut bytes)
        .map_err(|source| ExcelError::Io {
            path: "<xlsx>".to_string(),
            source,
        })?;
    if bytes.len() as u64 > limit {
        return Err(ExcelError::Parse {
            row: 0,
            message: format!("xlsx input exceeds {} MiB limit", limit / (1024 * 1024)),
        });
    }
    open_workbook_auto_from_rs(std::io::Cursor::new(bytes)).map_err(|err| ExcelError::Parse {
        row: 0,
        message: err.to_string(),
    })
}

/// Read the requested sheet (or the first sheet when `sheet` is `None`).
///
/// Returns `SheetNotFound` when a named sheet is absent. The returned
/// [`RawSheet`] yields every physical row (header included) and reports the
/// range height as its known total.
pub(crate) fn read(reader: Box<dyn Read>, sheet: Option<&str>) -> Result<RawSheet> {
    let mut workbook = open(reader)?;
    let names = workbook.sheet_names();
    let target = match sheet {
        Some(name) => {
            if !names.iter().any(|n| n == name) {
                return Err(ExcelError::SheetNotFound(name.to_string()));
            }
            name.to_string()
        }
        None => names.first().cloned().ok_or_else(|| ExcelError::Parse {
            row: 0,
            message: "workbook contains no worksheets".to_string(),
        })?,
    };

    let range = workbook
        .worksheet_range(&target)
        .map_err(|err| ExcelError::Parse {
            row: 0,
            message: err.to_string(),
        })?;

    let total = range.height() as u64;
    let rows: Vec<Vec<Value>> = range
        .rows()
        .map(|row| row.iter().map(data_to_value).collect())
        .collect();

    Ok(RawSheet::from_vec(rows, Some(total)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `Data` → `Value` mapping covers every typed cell variant.
    #[test]
    fn data_mapping() {
        assert_eq!(data_to_value(&Data::Int(7)), Value::from(7));
        assert_eq!(data_to_value(&Data::Bool(true)), Value::from(true));
        assert_eq!(data_to_value(&Data::String("x".into())), Value::from("x"));
        assert_eq!(data_to_value(&Data::Empty), Value::Null);
        assert_eq!(
            data_to_value(&Data::Error(calamine::CellErrorType::Value)),
            Value::Null
        );
    }

    /// An over-limit source yields a typed parse error rather than buffering it
    /// whole. A small limit keeps the test cheap; an unbounded reader stands in
    /// for a hostile stream.
    #[test]
    fn over_limit_source_is_typed_error() {
        let result = open_with_limit(Box::new(std::io::repeat(0u8)), 1024);
        match result {
            Err(ExcelError::Parse { row, message }) => {
                assert_eq!(row, 0);
                assert!(message.contains("limit"), "message was {message:?}");
            }
            Err(other) => panic!("expected Parse error, got {other:?}"),
            Ok(_) => panic!("over-limit source must be rejected"),
        }
    }

    /// A source at exactly the limit is read and handed to the parser (which
    /// rejects the non-zip bytes, proving the guard itself did not trip).
    #[test]
    fn at_limit_source_is_not_rejected_by_guard() {
        let payload = vec![0u8; 1024];
        let result = open_with_limit(Box::new(std::io::Cursor::new(payload)), 1024);
        match result {
            Err(ExcelError::Parse { message, .. }) => {
                assert!(!message.contains("limit"), "guard tripped: {message:?}");
            }
            Err(other) => panic!("expected Parse error, got {other:?}"),
            Ok(_) => panic!("non-zip bytes must fail to parse"),
        }
    }
}
