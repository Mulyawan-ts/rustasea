//! Persistent database batch repository and lifecycle callbacks (LARAVEL-013).
//!
//! Positive: a 10-job batch is created in `job_batches`, dispatched, and once
//! every job reaches a terminal outcome the `then` (and `finally`) callback
//! fires. Negative: a failing job records its id in `failed_job_ids`, fires the
//! `catch` callback, and suppresses `then`. Also covers the atomic pending
//! decrement under concurrency (no lost updates, never below zero).
//!
//! ## Shared-state isolation
//!
//! The batch repository is process-wide (matching the registry pattern), so a
//! single SQLite pool backs every test. Isolation comes structurally from
//! unique batch ids and unique job types/connections per test, so parallel
//! execution never collides.
//!
//! The pool is backed by a *file* rather than `sqlite::memory:`. sqlx returns a
//! checked-out connection to its pool from a task spawned on the *current*
//! tokio runtime; each `#[tokio::test]` owns a fresh runtime, so when the first
//! test finishes and its runtime is dropped the in-flight return is cancelled
//! and the pinned single connection (the whole `:memory:` database, including
//! the migrated `job_batches` table) is closed. Every later test then opens a
//! brand-new empty in-memory database. A file-backed database outlives
//! individual runtimes, so the migration applied once inside the shared
//! initializer stays visible to all tests in the process.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use rustasea_orm::{DbPool, PoolSettings};
use rustasea_queue::{
    on_catch, on_finally, on_then, record_batch_outcome, set_batch_repository,
    DatabaseBatchRepository, DispatchHandle, Job, JobError, Queue,
};
use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;

/// Process-wide batch repository backed by one file-based SQLite pool.
static REPO: OnceCell<Arc<DatabaseBatchRepository>> = OnceCell::const_new();

/// Install (once) and return the shared batch repository.
///
/// The migration runs inside the same initializer that opens the pool, so the
/// `job_batches` table exists before any test touches the repository. The
/// database is file-backed because an in-memory pool would be torn down with
/// the runtime of the first test that opened it (see the module docs).
async fn repo() -> Arc<DatabaseBatchRepository> {
    REPO.get_or_init(|| async {
        // One process-scoped path, wiped up front so a stale file from a
        // previous run cannot make the pool observe leftover rows.
        let path = std::env::temp_dir().join(format!(
            "rustasea-queue-job-batches-{}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let url = format!("sqlite://{}?mode=rwc", path.display());
        // Pin the file-backed pool to a single connection, mirroring how the
        // in-memory pool serializes access. Without this, concurrent writers
        // each open their own connection to the same file and SQLite reports
        // `database is locked` instead of the pool queueing the work.
        let settings = PoolSettings {
            max_connections: 1,
            ..PoolSettings::default()
        };
        let pool = DbPool::connect_with_settings(&url, settings)
            .await
            .expect("file-backed sqlite pool");
        rustasea_queue::queue_migrator()
            .run(&pool)
            .await
            .expect("queue migrations");
        let repo = Arc::new(DatabaseBatchRepository::new(pool));
        set_batch_repository(repo.clone());
        repo
    })
    .await
    .clone()
}

/// A batchable job that always succeeds.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct BatchSuccessJob {
    /// Sequence number, distinct per dispatched job.
    seq: u32,
}

#[async_trait::async_trait]
impl Job for BatchSuccessJob {
    /// No-op success body.
    async fn handle(self) -> std::result::Result<(), JobError> {
        Ok(())
    }
}

/// A batchable job that always fails.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct BatchFailJob {
    /// Marker field (unused).
    id: u32,
}

#[async_trait::async_trait]
impl Job for BatchFailJob {
    /// Always returns a domain error so the job dead-letters.
    async fn handle(self) -> std::result::Result<(), JobError> {
        Err(JobError::Exception("boom".to_string()))
    }
}

/// Positive: 10 sync jobs tagged with a batch id finish and fire `then`.
#[tokio::test]
async fn batch_of_ten_runs_and_fires_then() {
    let repo = repo().await;
    Queue::route::<BatchSuccessJob>("sync", "batch-positive").expect("route once");

    let batch = repo
        .create("import", 10, serde_json::json!({}))
        .await
        .expect("create batch");

    let then_calls = Arc::new(AtomicUsize::new(0));
    let finally_calls = Arc::new(AtomicUsize::new(0));
    {
        let then_calls = then_calls.clone();
        on_then(&batch.id, move |_record| {
            then_calls.fetch_add(1, Ordering::SeqCst);
        });
    }
    {
        let finally_calls = finally_calls.clone();
        on_finally(&batch.id, move |_record| {
            finally_calls.fetch_add(1, Ordering::SeqCst);
        });
    }

    for seq in 0..10 {
        let handle: DispatchHandle = BatchSuccessJob { seq }
            .dispatch()
            .on_batch(batch.id.clone());
        handle.dispatch().await.expect("dispatch batch job");
    }

    let stored = repo
        .find(&batch.id)
        .await
        .expect("find batch")
        .expect("batch row");
    assert_eq!(stored.pending_jobs, 0, "every job must decrement pending");
    assert_eq!(stored.failed_jobs, 0, "no failures expected");
    assert!(stored.is_finished(), "batch must be finished");
    assert_eq!(
        then_calls.load(Ordering::SeqCst),
        1,
        "then fires exactly once"
    );
    assert_eq!(
        finally_calls.load(Ordering::SeqCst),
        1,
        "finally fires exactly once"
    );
}

/// Negative: a failing job records its id, fires `catch`, and suppresses `then`.
#[tokio::test]
async fn failure_records_id_and_fires_catch() {
    let repo = repo().await;

    let batch = repo
        .create("import-with-failure", 2, serde_json::json!({}))
        .await
        .expect("create batch");

    let catch_calls = Arc::new(AtomicUsize::new(0));
    let then_calls = Arc::new(AtomicUsize::new(0));
    let finally_calls = Arc::new(AtomicUsize::new(0));
    {
        let catch_calls = catch_calls.clone();
        on_catch(&batch.id, move |_record| {
            catch_calls.fetch_add(1, Ordering::SeqCst);
        });
    }
    {
        let then_calls = then_calls.clone();
        on_then(&batch.id, move |_record| {
            then_calls.fetch_add(1, Ordering::SeqCst);
        });
    }
    {
        let finally_calls = finally_calls.clone();
        on_finally(&batch.id, move |_record| {
            finally_calls.fetch_add(1, Ordering::SeqCst);
        });
    }

    // One success, then a failure carrying an explicit job id.
    record_batch_outcome(&batch.id, None)
        .await
        .expect("success outcome");
    let after_failure = record_batch_outcome(&batch.id, Some("job-xyz".to_string()))
        .await
        .expect("failure outcome")
        .expect("batch row");

    assert_eq!(after_failure.failed_jobs, 1, "one failure recorded");
    assert_eq!(
        after_failure.failed_job_ids,
        vec!["job-xyz".to_string()],
        "the failed job id must be recorded"
    );
    assert_eq!(after_failure.pending_jobs, 0, "batch drained");
    assert!(after_failure.is_finished(), "batch finished");
    assert_eq!(catch_calls.load(Ordering::SeqCst), 1, "catch fires once");
    assert_eq!(
        then_calls.load(Ordering::SeqCst),
        0,
        "then must not fire when a job failed"
    );
    assert_eq!(
        finally_calls.load(Ordering::SeqCst),
        1,
        "finally always fires"
    );
}

/// The sync dispatch hook records a failing batchable job's outcome.
#[tokio::test]
async fn sync_failing_batch_job_fires_catch() {
    let repo = repo().await;
    Queue::route::<BatchFailJob>("sync", "batch-negative").expect("route once");

    let batch = repo
        .create("sync-failure", 1, serde_json::json!({}))
        .await
        .expect("create batch");

    let catch_calls = Arc::new(AtomicUsize::new(0));
    let finally_calls = Arc::new(AtomicUsize::new(0));
    {
        let catch_calls = catch_calls.clone();
        on_catch(&batch.id, move |_record| {
            catch_calls.fetch_add(1, Ordering::SeqCst);
        });
    }
    {
        let finally_calls = finally_calls.clone();
        on_finally(&batch.id, move |_record| {
            finally_calls.fetch_add(1, Ordering::SeqCst);
        });
    }

    BatchFailJob { id: 1 }
        .dispatch()
        .on_batch(batch.id.clone())
        .dispatch()
        .await
        .expect("sync dispatch");

    let stored = repo.find(&batch.id).await.expect("find").expect("row");
    assert_eq!(stored.failed_jobs, 1, "failure recorded by the sync hook");
    assert!(
        !stored.failed_job_ids.is_empty(),
        "a failed job id must be recorded"
    );
    assert!(stored.is_finished(), "single-job batch finished");
    assert_eq!(catch_calls.load(Ordering::SeqCst), 1, "catch fires once");
    assert_eq!(
        finally_calls.load(Ordering::SeqCst),
        1,
        "finally fires once"
    );
}

/// Atomic decrement: concurrent completions never lose an update or go negative.
#[tokio::test]
async fn concurrent_decrements_are_atomic() {
    let repo = repo().await;
    let batch = repo
        .create("concurrent", 10, serde_json::json!({}))
        .await
        .expect("create batch");

    let then_calls = Arc::new(AtomicUsize::new(0));
    {
        let then_calls = then_calls.clone();
        on_then(&batch.id, move |_record| {
            then_calls.fetch_add(1, Ordering::SeqCst);
        });
    }

    let mut tasks = Vec::new();
    for _ in 0..10 {
        let id = batch.id.clone();
        tasks.push(tokio::spawn(async move {
            record_batch_outcome(&id, None).await.expect("outcome");
        }));
    }
    for task in tasks {
        task.await.expect("join");
    }

    let stored = repo.find(&batch.id).await.expect("find").expect("row");
    assert_eq!(stored.pending_jobs, 0, "exactly ten decrements landed");
    assert_eq!(stored.failed_jobs, 0, "no failures");
    assert!(stored.is_finished(), "batch finished");
    assert_eq!(
        then_calls.load(Ordering::SeqCst),
        1,
        "then fires exactly once under concurrency"
    );
}

/// `record_batch_outcome` is a no-op for an unknown batch id.
#[tokio::test]
async fn unknown_batch_is_a_noop() {
    let _ = repo().await;
    let result = record_batch_outcome("00000000-0000-0000-0000-000000000000", None)
        .await
        .expect("no error");
    assert!(result.is_none(), "unknown batch yields no record");
}

/// Concurrent failures retain every id and fire `catch` exactly once.
///
/// Each failing job carries a distinct id; the compare-and-swap append must not
/// lose any under contention, and the in-transaction first-failure flag must
/// gate the `catch` callback to a single fire.
#[tokio::test]
async fn concurrent_failures_retain_all_ids_and_fire_catch_once() {
    let repo = repo().await;
    let total = 8usize;
    let batch = repo
        .create("concurrent-failures", total, serde_json::json!({}))
        .await
        .expect("create batch");

    let catch_calls = Arc::new(AtomicUsize::new(0));
    {
        let catch_calls = catch_calls.clone();
        on_catch(&batch.id, move |_record| {
            catch_calls.fetch_add(1, Ordering::SeqCst);
        });
    }

    let mut tasks = Vec::new();
    for seq in 0..total {
        let id = batch.id.clone();
        tasks.push(tokio::spawn(async move {
            record_batch_outcome(&id, Some(format!("failed-{seq}")))
                .await
                .expect("failure outcome");
        }));
    }
    for task in tasks {
        task.await.expect("join");
    }

    let stored = repo.find(&batch.id).await.expect("find").expect("row");
    assert_eq!(
        stored.failed_jobs, total as i64,
        "every failure incremented the counter"
    );
    assert_eq!(
        stored.failed_job_ids.len(),
        total,
        "no failed id lost under contention"
    );
    let mut ids = stored.failed_job_ids.clone();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), total, "all failed ids are distinct and retained");
    assert_eq!(
        stored.failed_jobs as usize,
        stored.failed_job_ids.len(),
        "invariant failed_jobs == failed_job_ids.len()"
    );
    assert!(stored.is_finished(), "batch drained and finished");
    assert_eq!(
        catch_calls.load(Ordering::SeqCst),
        1,
        "catch fires exactly once across concurrent failures"
    );
}

/// Concurrent `finish` calls claim the completion transition exactly once.
#[tokio::test]
async fn concurrent_finish_claims_exactly_once() {
    let repo = repo().await;
    let batch = repo
        .create("concurrent-finish", 1, serde_json::json!({}))
        .await
        .expect("create batch");

    let mut tasks = Vec::new();
    for _ in 0..8 {
        let id = batch.id.clone();
        let repo = repo.clone();
        tasks.push(tokio::spawn(async move {
            repo.finish(&id).await.expect("finish").expect("row").1
        }));
    }
    let mut winners = 0usize;
    for task in tasks {
        if task.await.expect("join") {
            winners += 1;
        }
    }
    assert_eq!(winners, 1, "exactly one caller claims the completion");
    let stored = repo.find(&batch.id).await.expect("find").expect("row");
    assert!(stored.is_finished(), "batch finished");
}
