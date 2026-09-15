//! Streaming CSV reader — one physical row per `read_record` call.
//!
//! The reader owns a boxed `Read` source and yields [`Value`] rows lazily, so a
//! multi-million-row file is never materialized. Each field is coerced through
//! [`coerce_csv_cell`](crate::value::coerce_csv_cell); the total row count is
//! unknown up front (`None`). A UTF-8 byte-order mark (`\u{feff}`) at the very
//! start of the stream is stripped from the first cell so a BOM-prefixed header
//! (`\u{feff}name`) is not treated as a distinct column.

use std::io::Read;

use serde_json::Value;

use crate::error::Result;
use crate::reader::RawSheet;
use crate::value::coerce_csv_cell;

/// A lazy CSV row iterator owning its source reader.
struct CsvSheet<R: Read> {
    /// Underlying csv reader; owns the source.
    reader: csv::Reader<R>,
    /// Reused record buffer (avoids a per-row allocation).
    buffer: csv::StringRecord,
    /// Whether the first physical row has been yielded (BOM stripping).
    started: bool,
}

impl<R: Read> Iterator for CsvSheet<R> {
    type Item = Result<Vec<Value>>;

    /// Read the next physical row, coercing each field to a JSON value.
    fn next(&mut self) -> Option<Self::Item> {
        match self.reader.read_record(&mut self.buffer) {
            Ok(false) => None,
            Ok(true) => {
                let mut cells: Vec<Value> = self.buffer.iter().map(coerce_csv_cell).collect();
                if !self.started {
                    self.started = true;
                    strip_bom(&mut cells);
                }
                Some(Ok(cells))
            }
            Err(err) => Some(Err(err.into())),
        }
    }
}

/// Strip a leading UTF-8 BOM from the first cell of the first row.
fn strip_bom(cells: &mut [Value]) {
    if let Some(Value::String(first)) = cells.first_mut() {
        if let Some(stripped) = first.strip_prefix('\u{feff}') {
            *first = stripped.to_string();
        }
    }
}

/// Build a streaming [`RawSheet`] over `reader` using `delimiter`.
pub(crate) fn read<R: Read + 'static>(reader: R, delimiter: u8) -> RawSheet {
    let csv_reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .flexible(true)
        .has_headers(false)
        .from_reader(reader);
    RawSheet {
        rows: Box::new(CsvSheet {
            reader: csv_reader,
            buffer: csv::StringRecord::new(),
            started: false,
        }),
        total: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Rows stream out in order with coerced cells.
    #[test]
    fn streams_and_coerces() {
        let data = b"name,age,active\nada,36,true\ngrace,45,false\n";
        let sheet = read(Box::new(std::io::Cursor::new(data.to_vec())), b',');
        let rows: Vec<Vec<Value>> = sheet.rows.map(|r| r.unwrap()).collect();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], vec![json!("name"), json!("age"), json!("active")]);
        assert_eq!(rows[1], vec![json!("ada"), json!(36), json!(true)]);
        assert_eq!(rows[2][2], json!(false));
        assert_eq!(sheet.total, None);
    }

    /// The tab delimiter is honoured for `.tsv` sources.
    #[test]
    fn tab_delimiter() {
        let data = b"a\tb\n1\t2\n";
        let sheet = read(Box::new(std::io::Cursor::new(data.to_vec())), b'\t');
        let rows: Vec<Vec<Value>> = sheet.rows.map(|r| r.unwrap()).collect();
        assert_eq!(rows[1], vec![json!(1), json!(2)]);
    }

    /// A leading UTF-8 BOM is stripped from the first header cell only.
    #[test]
    fn strips_leading_bom() {
        let mut data = Vec::new();
        data.extend_from_slice("\u{feff}".as_bytes());
        data.extend_from_slice(b"name,age\nada,36\n");
        let sheet = read(Box::new(std::io::Cursor::new(data)), b',');
        let rows: Vec<Vec<Value>> = sheet.rows.map(|r| r.unwrap()).collect();
        assert_eq!(rows[0], vec![json!("name"), json!("age")]);
        assert_eq!(rows[1], vec![json!("ada"), json!(36)]);
    }

    /// A BOM inside a later cell is preserved (only the stream start is special).
    #[test]
    fn preserves_inner_bom() {
        let data = "a,b\nx,\u{feff}y\n".as_bytes().to_vec();
        let sheet = read(Box::new(std::io::Cursor::new(data)), b',');
        let rows: Vec<Vec<Value>> = sheet.rows.map(|r| r.unwrap()).collect();
        assert_eq!(rows[1], vec![json!("x"), json!("\u{feff}y")]);
    }
}
