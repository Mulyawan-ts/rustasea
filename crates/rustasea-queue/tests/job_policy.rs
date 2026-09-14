//! Runtime consumer for `#[tries]`/`#[backoff]`/`#[timeout]` (LARAVEL-011).
//!
//! Positive: a transiently failing job retries up to its declared budget and
//! succeeds on a later run. Negative: a permanently failing job is dead-lettered
//! to `failed_jobs` after exactly `#[tries(n)]` attempts. Also covers the
//! backoff availability window and per-attempt timeout enforcement.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use rustasea_queue::{
    failed_jobs, register_job_with_policy, run_worker, Job, JobError, JobPayload, JobPolicy,
    QueueDriver, SyncDriver,
};
use serde::{Deserialize, Serialize};

/// Process-wide execution counter keyed by a per-job `key`.
static ATTEMPTS: OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();

/// Access the attempt counter, initializing it on first use.
fn attempts() -> &'static Mutex<HashMap<String, u32>> {
    ATTEMPTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Increment and return the execution count for `key` (1-based).
fn bump(key: &str) -> u32 {
    let mut guard = attempts().lock().unwrap_or_else(|p| p.into_inner());
    let entry = guard.entry(key.to_string()).or_insert(0);
    *entry += 1;
    *entry
}

/// Job that fails until its `succeed_on`-th execution, then succeeds.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FlakyJob {
    /// Stable key into the attempt counter.
    key: String,
    /// 1-based execution number that should succeed.
    succeed_on: u32,
}

#[async_trait::async_trait]
impl Job for FlakyJob {
    /// Fail transiently until the declared success attempt.
    async fn handle(self) -> std::result::Result<(), JobError> {
        let n = bump(&self.key);
        if n >= self.succeed_on {
            Ok(())
        } else {
            Err(JobError::Exception(format!("transient failure #{n}")))
        }
    }
}

/// Job that always fails (permanent domain error).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DoomedJob {
    /// Stable key into the attempt counter.
    key: String,
}

#[async_trait::async_trait]
impl Job for DoomedJob {
    /// Always fail with a stable domain exception.
    async fn handle(self) -> std::result::Result<(), JobError> {
        bump(&self.key);
        Err(JobError::Exception("permanent failure".to_string()))
    }
}

/// Permanent-failure job variant used to observe the released backoff delay.
///
/// A distinct type keeps its registered policy from colliding with
/// [`DoomedJob`]'s (the handler registry is process-global and tests run in
/// parallel).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct BackoffDoomedJob {
    /// Stable key into the attempt counter.
    key: String,
}

#[async_trait::async_trait]
impl Job for BackoffDoomedJob {
    /// Always fail so the worker releases a delayed retry.
    async fn handle(self) -> std::result::Result<(), JobError> {
        bump(&self.key);
        Err(JobError::Exception("permanent failure".to_string()))
    }
}

/// Job that outlives its per-attempt timeout.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SlowJob {
    /// Stable key into the attempt counter.
    key: String,
}

#[async_trait::async_trait]
impl Job for SlowJob {
    /// Sleep well past the configured timeout before returning.
    async fn handle(self) -> std::result::Result<(), JobError> {
        bump(&self.key);
        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(())
    }
}

/// Build a payload carrying the serialized `job` body and its type key.
fn payload<J: Job + Serialize>(job: &J, queue: &str) -> JobPayload {
    JobPayload::new(
        queue,
        "sync",
        Some(std::any::type_name::<J>().to_string()),
        serde_json::to_value(job).expect("serialize job"),
    )
}

/// Count dead-lettered entries for `queue`.
fn failed_for(queue: &str) -> Vec<rustasea_queue::FailedJob> {
    failed_jobs()
        .into_iter()
        .filter(|f| f.queue == queue)
        .collect()
}

/// A transient failure retries up to `#[tries(n)]` and then succeeds.
#[tokio::test]
async fn transient_failure_retries_then_succeeds() {
    register_job_with_policy::<FlakyJob>(JobPolicy::new(3, Duration::from_secs(1), Duration::ZERO));

    let queue = "l011-transient";
    let key = "l011-transient-key";
    let driver = Arc::new(SyncDriver::new());
    driver
        .push(payload(
            &FlakyJob {
                key: key.to_string(),
                succeed_on: 3,
            },
            queue,
        ))
        .await
        .expect("push");

    let processed = run_worker(
        driver.clone() as Arc<dyn QueueDriver>,
        vec![queue.to_string()],
        16,
    )
    .await
    .expect("worker drains");

    // Three executions: two transient failures, then success on the third.
    assert_eq!(processed, 3, "retried twice then succeeded");
    assert_eq!(
        *attempts()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(key)
            .expect("attempts recorded"),
        3
    );
    assert_eq!(driver.pending_size(queue).await.expect("size"), 0);
    assert!(
        failed_for(queue).is_empty(),
        "a job that ultimately succeeds is never dead-lettered"
    );
}

/// A permanent failure is dead-lettered after exactly `#[tries(n)]` attempts.
#[tokio::test]
async fn permanent_failure_lands_in_failed_jobs_after_exact_tries() {
    register_job_with_policy::<DoomedJob>(JobPolicy::new(
        3,
        Duration::from_secs(1),
        Duration::ZERO,
    ));

    let queue = "l011-permanent";
    let key = "l011-permanent-key";
    let driver = Arc::new(SyncDriver::new());
    driver
        .push(payload(
            &DoomedJob {
                key: key.to_string(),
            },
            queue,
        ))
        .await
        .expect("push");

    let processed = run_worker(
        driver.clone() as Arc<dyn QueueDriver>,
        vec![queue.to_string()],
        16,
    )
    .await
    .expect("worker drains");

    // Exactly three executions (initial + two retries) then dead-letter.
    assert_eq!(processed, 3, "exactly tries attempts before failing");
    assert_eq!(
        *attempts()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(key)
            .expect("attempts recorded"),
        3
    );
    assert_eq!(driver.pending_size(queue).await.expect("size"), 0);

    let failed = failed_for(queue);
    assert_eq!(failed.len(), 1, "one dead-letter entry");
    assert!(
        failed[0].exception.contains("MaxAttemptsExceeded"),
        "exception trace names the exhausted budget: {}",
        failed[0].exception
    );
    assert!(
        failed[0].exception.contains("permanent failure"),
        "exception trace preserves the job error: {}",
        failed[0].exception
    );
}

/// A retry is released with the policy's backoff delay as `available_at`.
#[tokio::test]
async fn backoff_sets_delayed_retry_availability() {
    register_job_with_policy::<BackoffDoomedJob>(JobPolicy::new(
        3,
        Duration::from_secs(5),
        Duration::ZERO,
    ));

    let queue = "l011-backoff";
    let driver = Arc::new(SyncDriver::new());
    driver
        .push(payload(
            &BackoffDoomedJob {
                key: "l011-backoff-key".to_string(),
            },
            queue,
        ))
        .await
        .expect("push");

    let before = chrono::Utc::now();
    // Process exactly one attempt so the retry stays buffered for inspection.
    let processed = run_worker(
        driver.clone() as Arc<dyn QueueDriver>,
        vec![queue.to_string()],
        1,
    )
    .await
    .expect("worker drains one");
    assert_eq!(processed, 1);

    let retried = driver
        .pop(queue, Duration::ZERO)
        .await
        .expect("pop")
        .expect("retry was released");
    assert_eq!(retried.attempts, 2, "release advanced the attempt count");
    let available_at = retried.available_at.expect("delayed availability set");
    let delta = available_at - before;
    assert!(
        delta >= chrono::Duration::seconds(4) && delta <= chrono::Duration::seconds(6),
        "backoff of 5s reflected in available_at (got {delta})"
    );
}

/// `#[timeout(secs)]` fails an over-long attempt instead of hanging.
#[tokio::test]
async fn timeout_is_enforced_per_attempt() {
    register_job_with_policy::<SlowJob>(JobPolicy::new(
        1,
        Duration::ZERO,
        Duration::from_millis(20),
    ));

    let queue = "l011-timeout";
    let driver = Arc::new(SyncDriver::new());
    driver
        .push(payload(
            &SlowJob {
                key: "l011-timeout-key".to_string(),
            },
            queue,
        ))
        .await
        .expect("push");

    let processed = run_worker(
        driver.clone() as Arc<dyn QueueDriver>,
        vec![queue.to_string()],
        4,
    )
    .await
    .expect("worker drains");
    assert_eq!(processed, 1, "single attempt, no retries");

    let failed = failed_for(queue);
    assert_eq!(failed.len(), 1, "timed-out job is dead-lettered");
    assert!(
        failed[0].exception.contains("timed out"),
        "timeout is surfaced in the trace: {}",
        failed[0].exception
    );
}
