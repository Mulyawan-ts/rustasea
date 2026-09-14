//! Persistent database batch repository (`job_batches`) and lifecycle callbacks.
//!
//! Complements the in-memory [`crate::batch::BatchHandle`] accounting with a
//! durable [`DatabaseBatchRepository`] over the `job_batches` table (see
//! [`crate::migrations::CreateJobBatchesTable`]). A batch row is created once
//! per dispatch group; each job that carries the batch id in its
//! [`crate::job::JobPayload::batch_id`] metadata decrements the pending counter
//! when it reaches a terminal outcome.
//!
//! ## Concurrency
//!
//! Progress is decremented with an atomic
//! `UPDATE … SET pending_jobs = pending_jobs - 1 WHERE id = ?` guarded on
//! `pending_jobs > 0`, so two workers finishing concurrently can never drive the
//! counter below zero nor lose an update. A failure decrements pending and
//! increments `failed_jobs` in the same statement.
//!
//! The `failed_job_ids` JSON array is appended with an optimistic
//! compare-and-swap: the `UPDATE` is guarded on the previously-read text so a
//! concurrent append re-reads and retries rather than overwriting the loser's
//! id. First-failure detection and the completion claim both happen *inside*
//! their transaction/statement, so `catch`/`then`/`finally` fire exactly once
//! even when several workers observe the same transition.
//!
//! ## Callbacks and their limitation
//!
//! `then()` / `catch()` / `finally()` callbacks are **in-process**: they are
//! registered in the process-wide [`callbacks`] map keyed by batch id and fire
//! only in the process that observes the batch reach the relevant state. A
//! callback registered by the dispatching process will not fire in a *different*
//! worker process that happens to complete the batch. Distributed callback
//! delivery (broadcast/events) is deliberately out of scope here.

use std::sync::{Arc, OnceLock, RwLock};

use chrono::Utc;
use rustasea_orm::{DbPool, Value};

use crate::error::{QueueError, Result};

mod callbacks;
mod record;

pub use callbacks::{forget_batch_callbacks, on_catch, on_finally, on_then, BatchCallback};
pub use record::BatchRecord;

/// Default `job_batches` table name (matches `[queue.batching].table`).
pub const JOB_BATCHES_TABLE: &str = "job_batches";

/// Maximum optimistic append retries before a contended `failed_job_ids`
/// update is surfaced as a store error.
const MAX_APPEND_RETRIES: usize = 8;

/// Outcome of the failure-recording transaction, used to drive the optimistic
/// retry loop in [`DatabaseBatchRepository::record_failure`].
enum FailureTx {
    /// The batch row does not exist.
    Missing,
    /// The failure was appended (or the batch was already drained).
    Recorded {
        /// The row as of this transaction's commit.
        record: BatchRecord,
        /// Whether this call recorded the batch's first failure.
        first_failure: bool,
    },
    /// Another writer changed `failed_job_ids`; re-read and retry.
    Conflict,
}

/// Database-backed batch repository over the `job_batches` table.
///
/// All SQL uses `$n` positional binds through the [`rustasea_orm`] runtime API
/// so the same statements run on SQLite/Postgres/MySQL. Timestamps are stored as
/// RFC3339 UTC strings and compared lexicographically; `failed_job_ids` is a
/// JSON-encoded array of strings.
#[derive(Debug, Clone)]
pub struct DatabaseBatchRepository {
    pool: DbPool,
    table: String,
}

impl DatabaseBatchRepository {
    /// Create a repository over `pool` using the default `job_batches` table.
    pub fn new(pool: DbPool) -> Self {
        Self {
            pool,
            table: JOB_BATCHES_TABLE.to_string(),
        }
    }

    /// Create a repository over `pool` with a custom table name.
    pub fn with_table(pool: DbPool, table: impl Into<String>) -> Self {
        Self {
            pool,
            table: table.into(),
        }
    }

    /// Access the underlying pool.
    pub fn pool(&self) -> &DbPool {
        &self.pool
    }

    /// The backing table name.
    pub fn table(&self) -> &str {
        &self.table
    }

    /// Insert a fresh batch row and return it.
    ///
    /// `total_jobs` seeds both `total_jobs` and `pending_jobs`; `options` is
    /// serialized to JSON text.
    pub async fn create(
        &self,
        name: &str,
        total_jobs: usize,
        options: serde_json::Value,
    ) -> Result<BatchRecord> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        let total = i64::try_from(total_jobs).unwrap_or(i64::MAX);
        let options_text = serde_json::to_string(&options).unwrap_or_else(|_| "null".to_string());
        let sql = format!(
            "INSERT INTO {} \
             (id, name, total_jobs, pending_jobs, failed_jobs, failed_job_ids, options, created_at, finished_at, cancelled_at) \
             VALUES ($1, $2, $3, $4, 0, $5, $6, $7, NULL, NULL)",
            self.table
        );
        self.pool
            .execute_bind(
                &sql,
                &[
                    Value::Text(id.clone()),
                    Value::Text(name.to_string()),
                    Value::Int(total),
                    Value::Int(total),
                    Value::Text("[]".to_string()),
                    Value::Text(options_text),
                    Value::Text(record::rfc3339(now)),
                ],
            )
            .await?;
        Ok(BatchRecord {
            id,
            name: name.to_string(),
            total_jobs: total,
            pending_jobs: total,
            failed_jobs: 0,
            failed_job_ids: Vec::new(),
            options,
            created_at: now,
            finished_at: None,
            cancelled_at: None,
        })
    }

    /// Load a batch row by id, or `None` when absent.
    pub async fn find(&self, id: &str) -> Result<Option<BatchRecord>> {
        let sql = format!("SELECT * FROM {} WHERE id = $1", self.table);
        let rows = self
            .pool
            .fetch_json(&sql, &[Value::Text(id.to_string())])
            .await?;
        match rows.first() {
            Some(row) => record::batch_from_row(row),
            None => Ok(None),
        }
    }

    /// Atomically decrement `pending_jobs` by one (never below zero).
    ///
    /// Returns the updated row, or `None` when the batch does not exist.
    pub async fn decrement_pending(&self, id: &str) -> Result<Option<BatchRecord>> {
        let sql = format!(
            "UPDATE {} SET pending_jobs = pending_jobs - 1 \
             WHERE id = $1 AND pending_jobs > 0",
            self.table
        );
        self.pool
            .execute_bind(&sql, &[Value::Text(id.to_string())])
            .await?;
        self.find(id).await
    }

    /// Record a failed job: decrement pending, increment `failed_jobs`, append id.
    ///
    /// The whole read-modify-write runs inside one transaction. The append is a
    /// compare-and-swap guarded on the previously-read `failed_job_ids` text, so
    /// a concurrent failure re-reads and retries instead of overwriting the
    /// loser's id. The returned flag reports whether *this* call recorded the
    /// batch's first failure — computed from `failed_job_ids.is_empty()` inside
    /// the transaction, before the mutation — so the `catch` callback fires
    /// exactly once.
    ///
    /// Returns `(record, first_failure)`, or `None` when the batch does not
    /// exist. The invariant `failed_jobs == failed_job_ids.len()` is maintained
    /// because the counter and the array mutate in the same statement.
    pub async fn record_failure(
        &self,
        id: &str,
        failed_job_id: &str,
    ) -> Result<Option<(BatchRecord, bool)>> {
        let select = format!("SELECT * FROM {} WHERE id = $1", self.table);
        let update = format!(
            "UPDATE {} SET pending_jobs = pending_jobs - 1, \
             failed_jobs = failed_jobs + 1, failed_job_ids = $2 \
             WHERE id = $1 AND pending_jobs > 0 AND failed_job_ids = $3",
            self.table
        );

        for _ in 0..MAX_APPEND_RETRIES {
            let select = select.clone();
            let update = update.clone();
            let id_owned = id.to_string();
            let failed_owned = failed_job_id.to_string();
            let outcome = rustasea_orm::transaction(&self.pool, move |tx| {
                let select = select.clone();
                let update = update.clone();
                let id = id_owned.clone();
                let failed = failed_owned.clone();
                Box::pin(async move {
                    let rows = tx.fetch_json(&select, &[Value::Text(id.clone())]).await?;
                    let Some(row) = rows.first() else {
                        return Ok(FailureTx::Missing);
                    };
                    // A drained batch cannot record another failure.
                    if record::int_field(row, "pending_jobs") <= 0 {
                        return Ok(match record::batch_from_row(row).ok().flatten() {
                            Some(record) => FailureTx::Recorded {
                                record,
                                first_failure: false,
                            },
                            None => FailureTx::Missing,
                        });
                    }
                    let mut ids = record::parse_ids(row.get("failed_job_ids"));
                    let first_failure = ids.is_empty();
                    let prior_text = record::raw_ids_text(row).unwrap_or_else(|| "[]".to_string());
                    ids.push(failed);
                    let text = serde_json::to_string(&ids).unwrap_or_else(|_| "[]".to_string());
                    let affected = tx
                        .execute_bind(
                            &update,
                            &[
                                Value::Text(id.clone()),
                                Value::Text(text),
                                Value::Text(prior_text),
                            ],
                        )
                        .await?;
                    if affected == 0 {
                        // Another writer appended concurrently: re-read and retry.
                        return Ok(FailureTx::Conflict);
                    }
                    let rows = tx.fetch_json(&select, &[Value::Text(id)]).await?;
                    let Some(row) = rows.first() else {
                        return Ok(FailureTx::Missing);
                    };
                    Ok(match record::batch_from_row(row).ok().flatten() {
                        Some(record) => FailureTx::Recorded {
                            record,
                            first_failure,
                        },
                        None => FailureTx::Missing,
                    })
                })
            })
            .await?;

            match outcome {
                FailureTx::Missing => return Ok(None),
                FailureTx::Recorded {
                    record,
                    first_failure,
                } => return Ok(Some((record, first_failure))),
                FailureTx::Conflict => continue,
            }
        }

        Err(QueueError::StoreUnavailable(
            "batch failure append contended beyond retry budget".to_string(),
        ))
    }

    /// Stamp `finished_at` once (idempotent while `finished_at IS NULL`).
    ///
    /// Returns `(record, claimed)`: `claimed` is `true` only for the caller
    /// whose `UPDATE` actually transitioned `finished_at` from `NULL` (affected
    /// rows > 0), so exactly one worker wins the completion transition. A caller
    /// that raced behind the winner sees `claimed == false` and must not fire
    /// completion hooks.
    pub async fn finish(&self, id: &str) -> Result<Option<(BatchRecord, bool)>> {
        let sql = format!(
            "UPDATE {} SET finished_at = $2 WHERE id = $1 AND finished_at IS NULL",
            self.table
        );
        let affected = self
            .pool
            .execute_bind(
                &sql,
                &[
                    Value::Text(id.to_string()),
                    Value::Text(record::rfc3339(Utc::now())),
                ],
            )
            .await?;
        let claimed = affected > 0;
        match self.find(id).await? {
            Some(record) => Ok(Some((record, claimed))),
            None => Ok(None),
        }
    }

    /// Stamp `cancelled_at` once (idempotent while `cancelled_at IS NULL`).
    pub async fn cancel(&self, id: &str) -> Result<Option<BatchRecord>> {
        let sql = format!(
            "UPDATE {} SET cancelled_at = $2 WHERE id = $1 AND cancelled_at IS NULL",
            self.table
        );
        self.pool
            .execute_bind(
                &sql,
                &[
                    Value::Text(id.to_string()),
                    Value::Text(record::rfc3339(Utc::now())),
                ],
            )
            .await?;
        self.find(id).await
    }
}

/// Process-wide installed batch repository (`None` until wired at boot).
static REPOSITORY: OnceLock<RwLock<Option<Arc<DatabaseBatchRepository>>>> = OnceLock::new();

/// Access the repository cell, initializing it on first use.
fn repository_cell() -> &'static RwLock<Option<Arc<DatabaseBatchRepository>>> {
    REPOSITORY.get_or_init(|| RwLock::new(None))
}

/// Install the process-wide batch repository (boot-time).
///
/// Worker and sync dispatch hooks consult this to persist progress. The last
/// writer wins; pass `None` via [`clear_batch_repository`] to disable.
pub fn set_batch_repository(repo: Arc<DatabaseBatchRepository>) {
    if let Ok(mut guard) = repository_cell().write() {
        *guard = Some(repo);
    }
}

/// The installed batch repository, or `None` when batch persistence is off.
pub fn batch_repository() -> Option<Arc<DatabaseBatchRepository>> {
    repository_cell()
        .read()
        .ok()
        .and_then(|guard| guard.clone())
}

/// Remove the installed batch repository (tests / teardown).
pub fn clear_batch_repository() {
    if let Ok(mut guard) = repository_cell().write() {
        *guard = None;
    }
}

/// Record a terminal outcome for one job of `batch_id`.
///
/// When `failed_job_id` is `Some`, the failure is recorded (and the `catch`
/// callback fires on the first failure, detected atomically inside the failure
/// transaction); otherwise `pending_jobs` is decremented for a success. Once
/// `pending_jobs` reaches zero the batch is finished and the `then` (only when
/// there were no failures) and `finally` callbacks fire — but only in the caller
/// that wins the completion claim, so concurrent finalizers fire completion
/// exactly once.
///
/// A no-op returning `Ok(None)` when no repository is installed or the batch row
/// is missing, so the worker hook is always safe to call.
pub async fn record_batch_outcome(
    batch_id: &str,
    failed_job_id: Option<String>,
) -> Result<Option<BatchRecord>> {
    let Some(repo) = batch_repository() else {
        return Ok(None);
    };
    let (record, first_failure) = match failed_job_id.as_deref() {
        Some(failed) => match repo.record_failure(batch_id, failed).await? {
            Some((record, first_failure)) => (record, first_failure),
            None => return Ok(None),
        },
        None => match repo.decrement_pending(batch_id).await? {
            Some(record) => (record, false),
            None => return Ok(None),
        },
    };

    if first_failure {
        callbacks::fire_catch(batch_id, &record);
    }

    if record.pending_jobs <= 0 && !record.is_finished() {
        let (finished, claimed) = match repo.finish(batch_id).await? {
            Some((record, claimed)) => (record, claimed),
            None => return Ok(None),
        };
        if claimed {
            callbacks::fire_completion(batch_id, &finished);
        }
        return Ok(Some(finished));
    }
    Ok(Some(record))
}
