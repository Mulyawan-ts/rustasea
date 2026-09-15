//! Import/export reporting types — row errors, import report, export report.
//!
//! An import never aborts on a bad row: each failure becomes a [`RowError`] and
//! is aggregated into an [`ImportReport`]. An export returns an [`ExportReport`]
//! describing where the bytes landed and how many rows were written.

use rustasea_validation::ErrorBag;
use serde::Serialize;

/// A single row-level import failure (deserialization or validation).
///
/// `row` is the 1-based physical row number in the source file, counting the
/// header row when one is present (so the first data row of a headered file is
/// row 2). This matches how a spreadsheet user counts rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RowError {
    /// 1-based file row (header inclusive).
    pub row: u64,
    /// Offending field name, or `None` for whole-row failures.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// Stable machine code (`missing_field`, `type_mismatch`, or a rule code).
    pub code: String,
    /// Human-readable detail.
    pub message: String,
}

impl RowError {
    /// Build a whole-row error (no specific field).
    pub fn row(row: u64, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            row,
            field: None,
            code: code.into(),
            message: message.into(),
        }
    }

    /// Build a field-scoped error.
    pub fn field(
        row: u64,
        field: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            row,
            field: Some(field.into()),
            code: code.into(),
            message: message.into(),
        }
    }
}

/// Aggregate outcome of an import run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// Total data rows examined (excludes the header row).
    pub total: u64,
    /// Rows successfully deserialized and validated.
    pub imported: u64,
    /// Rows rejected (deserialization or validation failure).
    pub failed: u64,
    /// Per-row failure detail, in encounter order.
    pub errors: Vec<RowError>,
}

impl ImportReport {
    /// Create an empty report.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether every examined row imported successfully.
    pub fn is_ok(&self) -> bool {
        self.failed == 0
    }

    /// Fold the row errors into a validation [`ErrorBag`].
    ///
    /// Field-scoped errors are keyed by their field name; whole-row errors by
    /// `row {n}`. Messages carry the row number so a bag rendered to a client
    /// still identifies the offending line.
    pub fn error_bag(&self) -> ErrorBag {
        let mut bag = ErrorBag::new();
        for err in &self.errors {
            let key = err
                .field
                .clone()
                .unwrap_or_else(|| format!("row {}", err.row));
            bag.add_message(key, format!("row {}: {}", err.row, err.message));
        }
        bag
    }

    /// Render the row errors as CSV (`row,field,code,message`).
    ///
    /// A header line is always present, so an empty report yields just the
    /// header. Fields are quoted by the CSV writer when they contain commas or
    /// quotes.
    pub fn errors_csv(&self) -> String {
        let mut out = String::from("row,field,code,message\n");
        for err in &self.errors {
            let field = err.field.as_deref().unwrap_or("");
            let fields = [
                err.row.to_string(),
                field.to_string(),
                err.code.clone(),
                err.message.clone(),
            ];
            out.push_str(&csv_line(&fields));
            out.push('\n');
        }
        out
    }
}

/// Render one CSV record (RFC 4180 quoting) terminated by a newline.
fn csv_line(fields: &[String]) -> String {
    let mut wtr = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(Vec::new());
    // Writing to an in-memory buffer cannot fail; fall back to a manual join.
    if wtr.write_record(fields).is_err() {
        return fields.join(",");
    }
    match wtr.into_inner() {
        Ok(bytes) => String::from_utf8(bytes)
            .unwrap_or_else(|_| fields.join(","))
            .trim_end_matches('\n')
            .to_string(),
        Err(_) => fields.join(","),
    }
}

/// Outcome of an export terminal (`to_bytes`/`to_path`/`store`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportReport {
    /// Destination label: a filesystem path, storage key, or `<bytes>`.
    pub path: String,
    /// Number of data rows written (excludes the header row).
    pub rows: u64,
    /// Total serialized size in bytes.
    pub bytes: u64,
}

impl ExportReport {
    /// Create a report for `path` with the given row/byte counts.
    pub fn new(path: impl Into<String>, rows: u64, bytes: u64) -> Self {
        Self {
            path: path.into(),
            rows,
            bytes,
        }
    }

    /// Build a signed, time-limited download URL for this export.
    ///
    /// The path is normalized to a single leading slash, **percent-encoded**
    /// (RFC 3986 unreserved set, `/` preserved as the segment separator), and
    /// then signed with the signer's `sign_expiring` (an empty `extra` set). The
    /// returned URL is `{base}/{encoded_path}?{query}`.
    ///
    /// The encoded path is what the signer hashes and what the returned URL
    /// carries, so a verifier must call
    /// [`SignedUrlSigner::verify_now`](rustasea_auth::SignedUrlSigner::verify_now)
    /// with the **same encoded path** (the request path as received, e.g.
    /// `/exports/my%20report.xlsx`) — never the decoded form. `ttl_secs` bounds
    /// the link's validity.
    #[cfg(feature = "signed-url")]
    pub fn signed_url(
        &self,
        signer: &rustasea_auth::SignedUrlSigner,
        base_url: &str,
        ttl_secs: u64,
    ) -> std::result::Result<String, crate::ExcelError> {
        let path = format!("/{}", self.path.trim_start_matches('/'));
        let encoded = encode_path(&path);
        let query = signer
            .sign_expiring(&encoded, ttl_secs, &[])
            .map_err(|e| crate::ExcelError::SignedUrl(e.to_string()))?;
        let base = base_url.trim_end_matches('/');
        Ok(format!("{base}{encoded}?{query}"))
    }
}

/// Percent-encode a URL path, preserving the RFC 3986 unreserved set plus `/`
/// (so path segments stay readable). Every other byte — spaces, `%`, UTF-8
/// continuation bytes — becomes `%XX`, matching the encoding
/// [`SignedUrlSigner`](rustasea_auth::SignedUrlSigner) applies to the path in
/// its canonical payload.
#[cfg(feature = "signed-url")]
fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for &byte in path.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The error bag keys field errors by field and row errors by `row N`.
    #[test]
    fn error_bag_keys_by_field_or_row() {
        let mut report = ImportReport::new();
        report
            .errors
            .push(RowError::field(2, "email", "email", "bad"));
        report
            .errors
            .push(RowError::row(3, "type_mismatch", "wrong type"));
        let bag = report.error_bag();
        assert_eq!(bag.get("email").len(), 1);
        assert_eq!(bag.get("row 3").len(), 1);
        assert!(bag.get("email")[0].message.contains("row 2"));
    }

    /// `errors_csv` emits a stable header and one line per error.
    #[test]
    fn errors_csv_has_header_and_rows() {
        let mut report = ImportReport::new();
        report
            .errors
            .push(RowError::field(2, "email", "email", "invalid"));
        let csv = report.errors_csv();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "row,field,code,message");
        assert_eq!(lines.len(), 2);
        assert!(lines[1].starts_with("2,email,email,"));
    }

    /// An empty report still renders the header and reports `is_ok`.
    #[test]
    fn empty_report_is_ok() {
        let report = ImportReport::new();
        assert!(report.is_ok());
        assert_eq!(report.errors_csv().lines().count(), 1);
    }

    /// `csv_line` quotes fields containing commas.
    #[test]
    fn csv_line_quotes_commas() {
        let line = csv_line(&["a".into(), "b,c".into()]);
        assert_eq!(line, "a,\"b,c\"");
    }

    /// Paths with spaces/reserved characters are percent-encoded consistently:
    /// the URL carries the encoded path and verification with that same encoded
    /// path (what a server receives) succeeds.
    #[cfg(feature = "signed-url")]
    #[test]
    fn signed_url_encodes_path_and_verifies() {
        let signer = rustasea_auth::SignedUrlSigner::new("test-key".as_bytes().to_vec());
        let report = ExportReport::new("exports/my report.xlsx", 2, 32);
        let signed = report
            .signed_url(&signer, "https://files.example.test/", 3600)
            .unwrap();

        let (url, query) = signed.split_once('?').unwrap();
        assert_eq!(url, "https://files.example.test/exports/my%20report.xlsx");

        // The server verifies against the encoded request path.
        signer
            .verify_now("/exports/my%20report.xlsx", query)
            .expect("encoded path must verify");
    }

    /// Unreserved paths are unchanged, so existing plain-key links still verify.
    #[cfg(feature = "signed-url")]
    #[test]
    fn signed_url_leaves_plain_paths_unchanged() {
        let signer = rustasea_auth::SignedUrlSigner::new("test-key".as_bytes().to_vec());
        let report = ExportReport::new("exports/report.csv", 2, 32);
        let signed = report
            .signed_url(&signer, "https://files.example.test", 3600)
            .unwrap();
        let (url, query) = signed.split_once('?').unwrap();
        assert_eq!(url, "https://files.example.test/exports/report.csv");
        signer.verify_now("/exports/report.csv", query).unwrap();
    }
}
