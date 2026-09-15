//! Storage round-trip and queued-transform integration tests for
//! `rustasea-image` (ADOPT-024).
//!
//! These tests exercise the process-wide storage slot, so they take the shared
//! `common::lock_storage` guard. The queued-job test mirrors the file-backed
//! sqlite pattern from `rustasea-excel/tests/round_trip.rs`.

mod common;

use std::sync::Arc;

use common::{decode, gradient_png, lock_storage, temp_path};
use rustasea_image::{Format, Image, ImageError};
use rustasea_storage::LocalDisk;

/// A single-threaded runtime used to drive the async storage/queue calls.
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// `store`/`store_async` write to the installed disk and decode back.
#[test]
fn storage_round_trip() {
    let _guard = lock_storage();
    Image::clear_storage();

    let dir = temp_path("disk", "dir");
    let _ = std::fs::remove_dir_all(&dir);
    let storage: Arc<dyn rustasea_storage::Storage> = Arc::new(LocalDisk::new(&dir));
    assert!(Image::set_storage(storage.clone()));

    let bytes = gradient_png(200, 100);
    let report = Image::from_bytes(bytes)
        .thumbnail(64, 64)
        .encode(Format::Webp)
        .store("thumbs/a.webp")
        .unwrap();
    assert_eq!(report.path, "thumbs/a.webp");
    assert_eq!(report.format, Format::Webp);
    assert_eq!((report.width, report.height), (64, 64));

    let stored = runtime().block_on(storage.get("thumbs/a.webp")).unwrap();
    let decoded = decode(&stored);
    assert_eq!((decoded.width(), decoded.height()), (64, 64));
    assert_eq!(stored.len() as u64, report.bytes);

    Image::clear_storage();
}

/// `store` without a configured disk is a typed `NotConfigured`.
#[test]
fn store_without_storage_is_not_configured() {
    let _guard = lock_storage();
    Image::clear_storage();
    let bytes = gradient_png(20, 20);
    let err = Image::from_bytes(bytes).store("x/y.png").unwrap_err();
    assert!(matches!(err, ImageError::NotConfigured(_)));
}

/// `store` inside a multi-threaded runtime drives the sync bridge without
/// panicking (the `block_in_place` path).
///
/// Written as a synchronous test that drives a multi-thread runtime, so the
/// process-wide storage lock is held in a non-async frame (never across an
/// `await` in an `async fn`, which would be a clippy hazard).
#[test]
fn store_inside_multi_thread_runtime() {
    let _guard = lock_storage();
    Image::clear_storage();

    let dir = temp_path("disk-mt", "dir");
    let _ = std::fs::remove_dir_all(&dir);
    let storage: Arc<dyn rustasea_storage::Storage> = Arc::new(LocalDisk::new(&dir));
    assert!(Image::set_storage(storage.clone()));

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();

    let bytes = gradient_png(50, 50);
    rt.block_on(async {
        let report = Image::from_bytes(bytes)
            .fit(20, 20)
            .encode(Format::Png)
            .store("mt/out.png")
            .unwrap();
        assert_eq!((report.width, report.height), (20, 20));
        assert!(storage.exists("mt/out.png").await.unwrap());
    });

    Image::clear_storage();
}

/// A queued transform reads a source object, resizes, and writes the result.
#[cfg(feature = "queue")]
#[test]
fn queued_transform_round_trip() {
    use rustasea_image::ImageTransformJob;
    use rustasea_queue::{JobOutcome, Queue};

    let _guard = lock_storage();
    Image::clear_storage();

    let dir = temp_path("job-disk", "dir");
    let _ = std::fs::remove_dir_all(&dir);
    let storage: Arc<dyn rustasea_storage::Storage> = Arc::new(LocalDisk::new(&dir));
    assert!(Image::set_storage(storage.clone()));

    let rt = runtime();

    let source = gradient_png(500, 400);
    rt.block_on(storage.put("src/a.png", &source))
        .expect("put source");

    let db_path = temp_path("job-queue", "sqlite");
    let _ = std::fs::remove_file(&db_path);

    rt.block_on(async {
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
        Queue::connect::<ImageTransformJob>("database", "images", driver).expect("route");

        let job = ImageTransformJob::new("src/a.png", "thumbs/a.webp")
            .resize(100, 100)
            .format(Format::Webp)
            .quality(80);
        let outcome = Queue::dispatch_sync::<ImageTransformJob>(job)
            .await
            .expect("dispatch");
        assert!(matches!(outcome, JobOutcome::Succeeded));
    });

    let stored = rt
        .block_on(storage.get("thumbs/a.webp"))
        .expect("stored output");
    let decoded = decode(&stored);
    assert_eq!((decoded.width(), decoded.height()), (100, 100));

    Image::clear_storage();
}

/// A queued job whose storage slot is unset reports a typed error (no panic).
#[cfg(feature = "queue")]
#[test]
fn queued_transform_without_storage_fails() {
    use rustasea_image::ImageTransformJob;

    let _guard = lock_storage();
    Image::clear_storage();
    let err = runtime().block_on(async {
        ImageTransformJob::new("missing.png", "out.png")
            .resize(10, 10)
            .execute()
            .await
            .unwrap_err()
    });
    assert!(matches!(err, ImageError::NotConfigured(_)));
}
