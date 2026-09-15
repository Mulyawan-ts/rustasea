//! Unit tests for the export pipeline (split from `export.rs` to stay under the
//! 500-line cap).

use super::*;
use serde::Serialize;
use serde_json::json;

/// A row struct used across the export tests.
#[derive(Serialize)]
struct Row {
    /// Name column.
    name: String,
    /// Age column.
    age: i64,
}

/// CSV export derives headers from the first object's keys.
///
/// `serde_json::Value` stores objects in sorted key order, so derived headers
/// are alphabetical unless `.headers(..)` overrides them.
#[test]
fn csv_export_derives_headers() {
    let rows = vec![Row {
        name: "ada".into(),
        age: 36,
    }];
    let (bytes, report) = crate::Excel::export(rows).to_bytes().unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert_eq!(text, "age,name\n36,ada\n");
    assert_eq!(report.rows, 1);
}

/// Explicit headers control column order.
#[test]
fn explicit_headers_control_order() {
    let rows = vec![json!({"a": 1, "b": 2})];
    let (bytes, _) = crate::Excel::export(rows)
        .headers(vec!["b".into(), "a".into()])
        .to_bytes()
        .unwrap();
    assert_eq!(String::from_utf8(bytes).unwrap(), "b,a\n2,1\n");
}

/// Non-object rows without headers are a serialization error.
#[test]
fn non_object_rows_without_headers_error() {
    let rows = vec![json!(5), json!(6)];
    let err = crate::Excel::export(rows).to_bytes().unwrap_err();
    assert!(matches!(err, ExcelError::Serialization(_)));
}

/// Progress fires on chunk boundaries.
#[test]
fn progress_fires_on_boundaries() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    let seen = Arc::new(AtomicU64::new(0));
    let seen_cb = seen.clone();
    let rows = vec![json!({"n": 1}), json!({"n": 2}), json!({"n": 3})];
    let _ = crate::Excel::export(rows)
        .chunk_size(2)
        .on_progress(move |done, _total| {
            seen_cb.store(done, Ordering::SeqCst);
        })
        .to_bytes()
        .unwrap();
    assert_eq!(seen.load(Ordering::SeqCst), 2);
}

/// A 100k-row CSV `to_path` export streams to disk: the file has the header
/// plus every data row. The streaming path never builds a `Vec<Value>` of rows,
/// so this stays memory-bounded for a much larger source too.
#[test]
fn to_path_csv_streams_100k_rows() {
    let path = std::env::temp_dir().join(format!(
        "rustasea-excel-stream-export-{}.csv",
        std::process::id()
    ));
    // A lazy iterator keeps the rows out of memory until written.
    let rows = (0..100_000u64).map(|n| json!({ "n": n }));
    let report = crate::Excel::export(rows).to_path(&path).unwrap();
    assert_eq!(report.rows, 100_000);

    let text = std::fs::read_to_string(&path).unwrap();
    // Header + 100_000 data rows.
    assert_eq!(text.lines().count(), 100_001);
    assert!(text.starts_with("n\n0\n"));
    assert!(text.ends_with("99999\n"));
    assert_eq!(report.bytes, text.len() as u64);

    let _ = std::fs::remove_file(&path);
}
