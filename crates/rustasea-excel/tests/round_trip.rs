//! End-to-end tests for `rustasea-excel` (ADOPT-023).
//!
//! Covers CSV and XLSX round-trips, row-error reporting, streaming chunking on a
//! 100k-row file, the queued export + signed-URL flow, explicit headers, and
//! progress/chunk-size validation. Global state (the storage slot) is serialized
//! behind a static mutex so parallel tests never race on it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rustasea_excel::{Excel, ExcelError, ExportFormat};
use rustasea_storage::LocalDisk;
use serde::{Deserialize, Serialize};

/// Serializes every test that touches the process-wide storage slot.
static STORAGE_LOCK: Mutex<()> = Mutex::new(());

/// Acquire the storage-slot lock, tolerating a poisoned mutex.
fn lock_storage() -> std::sync::MutexGuard<'static, ()> {
    STORAGE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// A unique temp path for a test artifact (process-scoped, no fixtures dir).
///
/// The process id is embedded in the stem (not after the extension) so the
/// returned path keeps its real `.csv` / `.sqlite` extension.
fn temp_path(stem: &str, ext: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "rustasea-excel-{stem}-{}.{ext}",
        std::process::id()
    ))
}

/// A typed row that round-trips through both CSV and XLSX.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Row {
    /// Name column.
    name: String,
    /// Age column.
    age: i64,
    /// Score column.
    score: f64,
    /// Active column.
    active: bool,
    /// Optional note column.
    note: Option<String>,
}

/// Build a representative sample row set.
fn sample_rows() -> Vec<Row> {
    vec![
        Row {
            name: "ada".into(),
            age: 36,
            score: 9.5,
            active: true,
            note: Some("first".into()),
        },
        Row {
            name: "007".into(),
            age: 45,
            score: 3.25,
            active: false,
            note: None,
        },
    ]
}

/// CSV round-trip: export typed rows, import them back, compare values.
#[test]
fn csv_round_trip() {
    let rows = sample_rows();
    let (bytes, report) = Excel::export(rows.clone())
        .headers(vec![
            "name".into(),
            "age".into(),
            "score".into(),
            "active".into(),
            "note".into(),
        ])
        .to_bytes()
        .unwrap();
    assert_eq!(report.rows, 2);

    let imported: Vec<Row> = {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let rpt = Excel::import::<Row>()
            .from_reader(Box::new(std::io::Cursor::new(bytes)), ExportFormat::Csv)
            .on_chunk(move |chunk| {
                sink.lock().unwrap().extend_from_slice(chunk);
                Ok(())
            })
            .run()
            .unwrap();
        assert!(rpt.is_ok());
        Arc::try_unwrap(seen).unwrap().into_inner().unwrap()
    };
    assert_eq!(imported, rows);
    // The leading-zero name must stay a string, not become integer 7.
    assert_eq!(imported[1].name, "007");
}

/// XLSX round-trip via `.to_bytes` / `.from_reader`.
#[cfg(feature = "xlsx")]
#[test]
fn xlsx_round_trip() {
    let rows = sample_rows();
    let (bytes, report) = Excel::export(rows.clone())
        .headers(vec![
            "name".into(),
            "age".into(),
            "score".into(),
            "active".into(),
            "note".into(),
        ])
        .format(ExportFormat::Xlsx)
        .sheet("People")
        .to_bytes()
        .unwrap();
    assert!(report.bytes > 0);

    let imported: Vec<Row> = {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let rpt = Excel::import::<Row>()
            .from_reader(Box::new(std::io::Cursor::new(bytes)), ExportFormat::Xlsx)
            .on_chunk(move |chunk| {
                sink.lock().unwrap().extend_from_slice(chunk);
                Ok(())
            })
            .run()
            .unwrap();
        assert!(rpt.is_ok());
        Arc::try_unwrap(seen).unwrap().into_inner().unwrap()
    };
    assert_eq!(imported, rows);
}

/// Row errors: bad email rule + non-deserializable type, with report surfaces.
#[test]
fn row_errors_are_reported() {
    let rules = rustasea_validation::Rules::new().field("email", "required|email");
    let data = b"name,email,age\nada,ada@example.com,36\nbad,not-an-email,45\nnoemail,,50\ntyped,x@y.com,notanumber\n";
    let report = Excel::import::<serde_json::Value>()
        .from_reader(
            Box::new(std::io::Cursor::new(data.to_vec())),
            ExportFormat::Csv,
        )
        .rules(rules)
        .run()
        .unwrap();

    // Rows 1 and 4 have valid emails and import; rows 2 (malformed email) and
    // 3 (empty email) fail the `required|email` rule.
    assert_eq!(report.total, 4);
    assert_eq!(report.imported, 2, "rows 1 and 4 import");
    assert_eq!(report.failed, 2, "rows 2 and 3 fail validation");

    // 1-based row numbers include the header offset.
    let rows: Vec<u64> = report.errors.iter().map(|e| e.row).collect();
    assert_eq!(rows, vec![3, 4]);
    assert!(report
        .errors
        .iter()
        .all(|e| e.field.as_deref() == Some("email")));

    // error_bag keys by field; errors_csv has the stable header + rows.
    let bag = report.error_bag();
    assert_eq!(bag.get("email").len(), 2);
    let csv = report.errors_csv();
    assert!(csv.starts_with("row,field,code,message\n"));
    assert_eq!(csv.lines().count(), 3);
}

/// Streaming: a 100k-row CSV imports with a 1000-row chunk size in 100 chunks.
#[test]
fn streaming_chunked_import() {
    let path = temp_path("stream", "csv");
    {
        use std::io::Write;
        let file = std::fs::File::create(&path).unwrap();
        let mut writer = std::io::BufWriter::new(file);
        writeln!(writer, "n").unwrap();
        for i in 0..100_000u64 {
            writeln!(writer, "{i}").unwrap();
        }
        writer.flush().unwrap();
    }

    let chunks = Arc::new(AtomicU64::new(0));
    let counted = chunks.clone();
    let report = Excel::import::<Single>()
        .from_path(&path)
        .chunk_size(1000)
        .on_chunk(move |chunk| {
            assert!(chunk.len() <= 1000);
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .run()
        .unwrap();

    assert_eq!(report.imported, 100_000);
    assert!(report.is_ok());
    assert_eq!(chunks.load(Ordering::SeqCst), 100);

    let _ = std::fs::remove_file(&path);
}

/// A single-column row type used by the streaming test.
#[derive(Debug, Deserialize)]
struct Single {
    /// The only column.
    #[allow(dead_code)]
    n: u64,
}

/// Queued export + signed URL over a real sqlite queue and a local disk.
///
/// Written as a synchronous test that drives a private runtime, so the
/// process-wide storage lock is held in a non-async frame (never across an
/// `await` in an `async fn`, which would be a clippy hazard).
#[cfg(all(feature = "queue", feature = "signed-url"))]
#[test]
fn queued_export_and_signed_url() {
    use rustasea_excel::ExportJob;
    use rustasea_queue::{JobOutcome, Queue};

    let _guard = lock_storage();
    Excel::clear_storage();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    // A temp local disk and a file-backed sqlite queue (mirrors job_batches.rs).
    let disk_dir = temp_path("disk", "dir");
    let _ = std::fs::remove_dir_all(&disk_dir);
    let storage: Arc<dyn rustasea_storage::Storage> = Arc::new(LocalDisk::new(&disk_dir));
    assert!(Excel::set_storage(storage.clone()));

    let db_path = temp_path("queue", "sqlite");
    let _ = std::fs::remove_file(&db_path);

    runtime.block_on(async {
        let url = format!("sqlite://{}?mode=rwc", db_path.display());
        let pool = rustasea_orm::DbPool::connect_with_settings(
            &url,
            rustasea_orm::PoolSettings {
                max_connections: 1,
                ..rustasea_orm::PoolSettings::default()
            },
        )
        .await
        .expect("sqlite pool");
        rustasea_queue::queue_migrator()
            .run(&pool)
            .await
            .expect("queue migrations");
        let driver = Arc::new(rustasea_queue::DatabaseDriver::new(pool));
        Queue::connect::<ExportJob>("database", "exports", driver).expect("route export job");

        let job = ExportJob::new(
            "exports/report.csv",
            ExportFormat::Csv,
            vec!["name".into(), "age".into()],
        )
        .row(serde_json::json!({"name": "ada", "age": 36}))
        .unwrap()
        .row(serde_json::json!({"name": "grace", "age": 45}))
        .unwrap();

        let outcome = Queue::dispatch_sync::<ExportJob>(job)
            .await
            .expect("dispatch");
        assert!(matches!(outcome, JobOutcome::Succeeded));
        assert!(storage.exists("exports/report.csv").await.unwrap());

        let bytes = storage.get("exports/report.csv").await.unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("name,age\n"));
        assert!(text.contains("ada,36"));
    });

    // Build and verify a signed download URL for the stored object.
    let report = rustasea_excel::ExportReport::new("exports/report.csv", 2, 32);
    let signer = rustasea_auth::SignedUrlSigner::new("test-key".as_bytes().to_vec());
    let signed = report
        .signed_url(&signer, "https://files.example.test/", 3600)
        .unwrap();
    let (path, query) = signed.split_once('?').unwrap();
    assert_eq!(path, "https://files.example.test/exports/report.csv");
    signer
        .verify_now("/exports/report.csv", query)
        .expect("valid signature");

    // An already-expired link fails verification.
    let expired = report.signed_url(&signer, "https://x.test", 0).unwrap();
    let (_, exp_query) = expired.split_once('?').unwrap();
    let err = signer
        .verify("/exports/report.csv", exp_query, i64::MAX)
        .unwrap_err();
    assert!(matches!(err, rustasea_auth::SignedUrlError::Expired { .. }));

    Excel::clear_storage();
    let _ = std::fs::remove_dir_all(&disk_dir);
    let _ = std::fs::remove_file(&db_path);
}

/// Explicit `.columns` override; a missing non-nullable column becomes a
/// field-scoped row error, and extra source columns are ignored.
#[test]
fn explicit_columns_and_missing_field() {
    // The header declares only `name`; the `age` column the struct requires is
    // absent, so every row fails with a `missing field` error naming `age`.
    let data = b"name,extra\nada,ignored\nbob,ignored\n";
    let report = Excel::import::<Person>()
        .from_reader(
            Box::new(std::io::Cursor::new(data.to_vec())),
            ExportFormat::Csv,
        )
        .columns(vec!["name".to_string()])
        .run()
        .unwrap();
    assert_eq!(report.imported, 0);
    assert_eq!(report.failed, 2);
    // 1-based rows: first data row is row 2 (header offset).
    assert_eq!(report.errors[0].row, 2);
    assert_eq!(report.errors[0].field.as_deref(), Some("age"));
    assert_eq!(report.errors[0].code, "missing_field");
}

/// A source row with more columns than the declared header ignores the extras.
#[test]
fn extra_columns_are_ignored() {
    let data = b"name,age,extra\nada,36,ignored\n";
    let report = Excel::import::<Person>()
        .from_reader(
            Box::new(std::io::Cursor::new(data.to_vec())),
            ExportFormat::Csv,
        )
        .run()
        .unwrap();
    assert_eq!(report.imported, 1);
    assert!(report.is_ok());
}

/// A two-column person type used by the header test.
#[derive(Debug, Deserialize)]
struct Person {
    /// Name column.
    #[allow(dead_code)]
    name: String,
    /// Age column.
    #[allow(dead_code)]
    age: i64,
}

/// `on_progress` reports monotonically increasing counts.
#[test]
fn progress_is_monotonic() {
    let data = b"n\n1\n2\n3\n4\n5\n";
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let report = Excel::import::<Single>()
        .from_reader(
            Box::new(std::io::Cursor::new(data.to_vec())),
            ExportFormat::Csv,
        )
        .on_progress(move |done, total| {
            assert!(total.is_none(), "CSV progress total is unknown");
            sink.lock().unwrap().push(done);
        })
        .run()
        .unwrap();
    assert_eq!(report.imported, 5);
    let counts = Arc::try_unwrap(seen).unwrap().into_inner().unwrap();
    assert_eq!(counts, vec![1, 2, 3, 4, 5]);
    assert!(counts.windows(2).all(|w| w[0] < w[1]));
}

/// `chunk_size(0)` is a typed error.
#[test]
fn zero_chunk_size_is_error() {
    let data = b"n\n1\n";
    let err = Excel::import::<Single>()
        .from_reader(
            Box::new(std::io::Cursor::new(data.to_vec())),
            ExportFormat::Csv,
        )
        .chunk_size(0)
        .run()
        .unwrap_err();
    assert!(matches!(err, ExcelError::InvalidChunkSize(0)));
}

/// CSV-only build: a csv round-trip still works with default features disabled.
#[cfg(not(feature = "xlsx"))]
#[test]
fn csv_only_build_round_trips() {
    let rows = sample_rows();
    let (bytes, _) = Excel::export(rows)
        .headers(vec!["name".into()])
        .to_bytes()
        .unwrap();
    assert!(String::from_utf8(bytes).unwrap().starts_with("name\n"));
}

/// The synchronous `.store()` terminal writes through the global storage slot
/// and returns a report keyed by the object name.
#[test]
fn store_terminal_writes_to_storage() {
    let _guard = lock_storage();
    Excel::clear_storage();
    let disk_dir = temp_path("store-disk", "dir");
    let _ = std::fs::remove_dir_all(&disk_dir);
    let storage: Arc<dyn rustasea_storage::Storage> = Arc::new(LocalDisk::new(&disk_dir));
    assert!(Excel::set_storage(storage.clone()));

    let report = Excel::export(sample_rows())
        .headers(vec!["name".into(), "age".into()])
        .format(ExportFormat::Csv)
        .store("exports/store.csv")
        .unwrap();
    assert_eq!(report.path, "exports/store.csv");
    assert_eq!(report.rows, 2);

    // The file is readable back through the disk.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let exists = runtime
        .block_on(storage.exists("exports/store.csv"))
        .unwrap();
    assert!(exists);

    // With the slot cleared, `store` reports NotConfigured.
    Excel::clear_storage();
    let err = Excel::export(sample_rows())
        .headers(vec!["name".into()])
        .format(ExportFormat::Csv)
        .store("exports/none.csv")
        .unwrap_err();
    assert!(matches!(err, ExcelError::NotConfigured(_)));

    let _ = std::fs::remove_dir_all(&disk_dir);
}

// Async store-terminal tests live in a submodule so this file stays within the
// 500-line cap. An explicit `path` is required because this integration-test
// file is a crate root, not a `mod.rs`.
#[path = "round_trip/async_store.rs"]
mod async_store;
