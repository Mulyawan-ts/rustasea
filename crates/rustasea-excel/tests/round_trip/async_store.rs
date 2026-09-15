//! Async store-terminal tests (split from `round_trip.rs` to stay under the
//! 500-line cap). Shares the parent's fixtures (`lock_storage`, `temp_path`,
//! `sample_rows`) through `use super::*;`.

use super::*;

/// `.store()` from inside a multi-thread Tokio runtime must not panic
/// ("Cannot start a runtime from within a runtime"); it drives the write via
/// `block_in_place` on the ambient handle.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn store_terminal_inside_multithread_runtime() {
    let _guard = lock_storage();
    Excel::clear_storage();
    let disk_dir = temp_path("mt-store-disk", "dir");
    let _ = std::fs::remove_dir_all(&disk_dir);
    let storage: Arc<dyn rustasea_storage::Storage> = Arc::new(LocalDisk::new(&disk_dir));
    assert!(Excel::set_storage(storage));

    let report = Excel::export(sample_rows())
        .headers(vec!["name".into(), "age".into()])
        .format(ExportFormat::Csv)
        .store("exports/mt.csv")
        .expect("store must not panic inside an active runtime");
    assert_eq!(report.rows, 2);
    // The bytes landed on disk (LocalDisk writes straight to the temp dir).
    let written = std::fs::read_to_string(disk_dir.join("exports/mt.csv")).unwrap();
    assert!(written.starts_with("name,age\n"));

    Excel::clear_storage();
    let _ = std::fs::remove_dir_all(&disk_dir);
}

/// `.store_async()` awaits the storage write directly (no runtime bridging), so
/// it works inside a current-thread async test where `.store()` would block.
///
/// The `STORAGE_LOCK` guard is intentionally held across the `.await`: it
/// serializes the process-wide storage slot, which is exactly what the await
/// protects.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn store_async_terminal_in_async_context() {
    let _guard = lock_storage();
    Excel::clear_storage();
    let disk_dir = temp_path("async-store-disk", "dir");
    let _ = std::fs::remove_dir_all(&disk_dir);
    let storage: Arc<dyn rustasea_storage::Storage> = Arc::new(LocalDisk::new(&disk_dir));
    assert!(Excel::set_storage(storage));

    let report = Excel::export(sample_rows())
        .headers(vec!["name".into(), "age".into()])
        .format(ExportFormat::Csv)
        .store_async("exports/async.csv")
        .await
        .expect("store_async must write in async context");
    assert_eq!(report.path, "exports/async.csv");
    assert_eq!(report.rows, 2);
    let written = std::fs::read_to_string(disk_dir.join("exports/async.csv")).unwrap();
    assert!(written.starts_with("name,age\n"));

    Excel::clear_storage();
    let _ = std::fs::remove_dir_all(&disk_dir);
}

/// `.store()` inside a **current-thread** runtime (the default `#[tokio::test]`)
/// must not panic on `block_in_place` nor deadlock the single driver thread; the
/// sync bridge runs the write on a dedicated thread instead.
#[tokio::test]
async fn store_terminal_inside_current_thread_runtime() {
    let _guard = lock_storage();
    Excel::clear_storage();
    let disk_dir = temp_path("ct-store-disk", "dir");
    let _ = std::fs::remove_dir_all(&disk_dir);
    let storage: Arc<dyn rustasea_storage::Storage> = Arc::new(LocalDisk::new(&disk_dir));
    assert!(Excel::set_storage(storage));

    let report = Excel::export(sample_rows())
        .headers(vec!["name".into(), "age".into()])
        .format(ExportFormat::Csv)
        .store("exports/ct.csv")
        .expect("store must not panic/deadlock on a current-thread runtime");
    assert_eq!(report.rows, 2);
    let written = std::fs::read_to_string(disk_dir.join("exports/ct.csv")).unwrap();
    assert!(written.starts_with("name,age\n"));

    Excel::clear_storage();
    let _ = std::fs::remove_dir_all(&disk_dir);
}
