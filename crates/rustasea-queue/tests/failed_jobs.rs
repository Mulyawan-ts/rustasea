//! Failed-job management — `retry_failed` and `forget_failed` (ADOPT-021).
//!
//! Positive: a dead-lettered job is re-enqueued by `retry_failed` (row deleted
//! only after the push) and discarded by `forget_failed` (row deleted, no push).
//! Negative: forgetting an unknown id reports `false` and changes nothing.
//!
//! A single `sqlite::memory:` pool backs the database-driver cases: each
//! `#[tokio::test]` owns its runtime and pool, so the pinned in-memory database
//! lives exactly as long as the test that opened it.

use rustasea_orm::DbPool;
use rustasea_queue::{DatabaseDriver, JobId};

/// Build a migrated in-memory pool with a `jobs` + `failed_jobs` schema.
async fn pool() -> DbPool {
    let pool = DbPool::connect("sqlite::memory:")
        .await
        .expect("in-memory pool");
    rustasea_queue::queue_migrator()
        .run(&pool)
        .await
        .expect("queue migrations");
    pool
}

/// Insert a dead-letter row directly and return its id.
async fn seed_failed(pool: &DbPool, id: JobId) {
    let sql = "INSERT INTO failed_jobs (id, connection, queue, payload, exception, failed_at) \
               VALUES ($1, $2, $3, $4, $5, $6)";
    let payload = serde_json::json!({
        "queue": "default",
        "connection": "database",
        "available_at": null,
        "attempts": 1,
        "id": null,
        "job": "test::FailedJob",
        "batch_id": null,
        "payload": {}
    });
    pool.execute_bind(
        sql,
        &[
            rustasea_orm::Value::Text(id.to_string()),
            rustasea_orm::Value::Text("database".to_string()),
            rustasea_orm::Value::Text("default".to_string()),
            rustasea_orm::Value::Text(payload.to_string()),
            rustasea_orm::Value::Text("boom".to_string()),
            rustasea_orm::Value::Text(
                chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
            ),
        ],
    )
    .await
    .expect("seed failed row");
}

/// Count the rows in `failed_jobs`.
async fn failed_count(pool: &DbPool) -> usize {
    let rows = pool
        .fetch_json("SELECT COUNT(*) AS c FROM failed_jobs", &[])
        .await
        .expect("count");
    rows.first()
        .and_then(|r| r.get("c"))
        .and_then(|v| v.as_i64())
        .and_then(|v| usize::try_from(v).ok())
        .unwrap_or(0)
}

/// Positive: `forget_failed` deletes the row and reports `true`.
#[tokio::test]
async fn forget_failed_deletes_the_row() {
    let pool = pool().await;
    let driver = DatabaseDriver::new(pool.clone());
    let id = JobId::new();
    seed_failed(&pool, id).await;
    assert_eq!(failed_count(&pool).await, 1);

    let removed = driver.forget_failed(id).await.expect("forget");
    assert!(removed, "the matching row is removed");
    assert_eq!(failed_count(&pool).await, 0);
}

/// Negative: forgetting an unknown id reports `false` and changes nothing.
#[tokio::test]
async fn forget_failed_unknown_id_is_false() {
    let pool = pool().await;
    let driver = DatabaseDriver::new(pool.clone());
    let removed = driver.forget_failed(JobId::new()).await.expect("forget");
    assert!(!removed);
    assert_eq!(failed_count(&pool).await, 0);
}

/// Positive: `retry_failed` re-enqueues the job and deletes the failed row.
#[tokio::test]
async fn retry_failed_reenqueues_and_deletes() {
    let pool = pool().await;
    let driver = DatabaseDriver::new(pool.clone());
    let id = JobId::new();
    seed_failed(&pool, id).await;

    driver.retry_failed(id).await.expect("retry");

    assert_eq!(
        failed_count(&pool).await,
        0,
        "failed row deleted after push"
    );
    let jobs = pool
        .fetch_json("SELECT COUNT(*) AS c FROM jobs", &[])
        .await
        .expect("count jobs");
    let enqueued = jobs
        .first()
        .and_then(|r| r.get("c"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    assert_eq!(enqueued, 1, "the job is re-enqueued into jobs");
}
