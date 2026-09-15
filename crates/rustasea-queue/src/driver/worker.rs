//! Queue worker loop — pop, execute, ack, retry, or dead-letter.
//!
//! [`run_worker`] drains a set of queues through a [`QueueDriver`], resolving
//! each popped [`JobPayload`] to an erased handler via a type registry (or a
//! caller-supplied resolver). Execution goes through the existing
//! [`crate::job::run_erased`] timeout/retry path; success is `ack`ed, a
//! retryable failure is `release`d, and a permanent failure is dead-lettered.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use crate::driver::QueueDriver;
use crate::error::Result;
use crate::job::{run_erased, ConcreteJob, ErasedJob, FailedJob, Job, JobOutcome, JobPayload};
use crate::policy::JobPolicy;
use crate::unique::release_unique;

/// How long a worker blocks on `pop` before concluding a queue is drained.
const POP_TIMEOUT: Duration = Duration::from_millis(250);

/// Factory that rebuilds an erased handler from a serialized job body.
type HandlerFactory = Arc<dyn Fn(&serde_json::Value) -> Option<Arc<dyn ErasedJob>> + Send + Sync>;

/// Process-wide map from job type name to its deserializing handler factory.
static HANDLERS: OnceLock<RwLock<HashMap<&'static str, HandlerFactory>>> = OnceLock::new();

/// Access the handler registry, initializing it on first use.
fn handlers() -> &'static RwLock<HashMap<&'static str, HandlerFactory>> {
    HANDLERS.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Register a handler for job type `J` under its `type_name` key.
///
/// A worker resolving a payload whose `job` equals `J`'s type name deserializes
/// the JSON body into `J` and runs it through the standard retry/timeout path.
/// Registering the same type twice replaces the prior factory. The retry policy
/// is taken from the job's own `tries`/`backoff`/`timeout` accessors; use
/// [`register_job_with_policy`] to bind a declarative policy instead.
pub fn register_job<J>()
where
    J: Job + serde::de::DeserializeOwned + 'static,
{
    register_job_handler(
        std::any::type_name::<J>(),
        Arc::new(|body: &serde_json::Value| {
            let job: J = serde_json::from_value(body.clone()).ok()?;
            Some(Arc::new(ConcreteJob::new(job)))
        }),
    );
}

/// Register a handler for job type `J` that runs under an explicit `policy`.
///
/// This is the runtime binding point for the `#[tries]`/`#[backoff]`/
/// `#[timeout]` attribute bundle: an application builds a [`JobPolicy`] from the
/// macro-emitted `__RUSTASEA_TRIES_<Type>` / `__RUSTASEA_BACKOFF_SECS_<Type>` /
/// `__RUSTASEA_TIMEOUT_SECS_<Type>` consts (e.g.
/// `JobPolicy::from_seconds(__RUSTASEA_TRIES_Foo, __RUSTASEA_BACKOFF_SECS_Foo,
/// __RUSTASEA_TIMEOUT_SECS_Foo)`) and registers it here at boot. Rust has no
/// reflection, so this explicit registration — mirroring the `middleware_meta`
/// precedent — is how the worker learns the declarative policy per job type.
///
/// The policy takes precedence over the job's own accessors; registering the
/// same type twice replaces the prior factory.
pub fn register_job_with_policy<J>(policy: JobPolicy)
where
    J: Job + serde::de::DeserializeOwned + 'static,
{
    register_job_handler(
        std::any::type_name::<J>(),
        Arc::new(move |body: &serde_json::Value| {
            let job: J = serde_json::from_value(body.clone()).ok()?;
            Some(Arc::new(ConcreteJob::with_policy(job, policy)))
        }),
    );
}

/// Register a handler factory for an explicit `type_key`.
///
/// The factory receives the deserialized JSON body and returns the erased job
/// to run, or `None` when the body is invalid. Use this for dynamic types whose
/// key is not a Rust `type_name`.
pub fn register_job_handler(type_key: &'static str, factory: HandlerFactory) {
    if let Ok(mut guard) = handlers().write() {
        guard.insert(type_key, factory);
    }
}

/// Resolver backed by the global handler registry.
///
/// Looks up `payload.job` in the registry; returns `None` for an unregistered
/// or unnamed job, which the worker treats as a permanent failure.
pub fn default_resolver() -> impl Fn(&JobPayload) -> Option<Arc<dyn ErasedJob>> + Send + Sync {
    |payload: &JobPayload| {
        let key = payload.job.as_deref()?;
        let guard = handlers().read().ok()?;
        let factory = guard.get(key)?;
        factory(&payload.payload)
    }
}

/// Drain `queues` through `driver`, running at most `max_jobs` jobs.
///
/// Uses [`default_resolver`] to resolve handlers from the global registry.
/// Returns the number of jobs processed. Stops after `max_jobs`, when every
/// queue is drained, or on a store error.
pub async fn run_worker(
    driver: Arc<dyn QueueDriver>,
    queues: Vec<String>,
    max_jobs: usize,
) -> Result<usize> {
    run_worker_with(driver, queues, max_jobs, default_resolver()).await
}

/// Drain `queues` through `driver` using a custom `resolver`.
///
/// `resolver` maps a popped payload to its erased handler; returning `None`
/// dead-letters the job with a `no handler registered` exception. The loop
/// round-robins the queues, stopping once a full cycle yields no job.
pub async fn run_worker_with<F>(
    driver: Arc<dyn QueueDriver>,
    queues: Vec<String>,
    max_jobs: usize,
    resolver: F,
) -> Result<usize>
where
    F: Fn(&JobPayload) -> Option<Arc<dyn ErasedJob>> + Send + Sync,
{
    if queues.is_empty() || max_jobs == 0 {
        return Ok(0);
    }

    let mut processed = 0usize;
    let mut cursor = 0usize;
    let mut misses = 0usize;
    while processed < max_jobs {
        let queue = queues[cursor % queues.len()].clone();
        cursor += 1;
        // Stamp the queue as polled so the dashboard can show live workers
        // (ADOPT-021). Best-effort: the stamp can never fail the worker.
        crate::heartbeat::stamp(&queue);
        let Some(payload) = driver.pop(&queue, POP_TIMEOUT).await? else {
            // Stop only after a full cycle with no job (all queues drained).
            misses += 1;
            if misses >= queues.len() {
                break;
            }
            continue;
        };
        misses = 0;
        process_one(driver.as_ref(), &payload, &resolver).await?;
        processed += 1;
    }
    Ok(processed)
}

/// Build the dead-letter payload preserving the full job envelope.
///
/// Serializing the whole [`JobPayload`] (not just its inner body) keeps the job
/// type name and queue metadata, so `queue:retry` can rebuild a runnable
/// payload; falls back to the inner body if serialization fails.
fn dead_letter_payload(payload: &JobPayload) -> serde_json::Value {
    serde_json::to_value(payload).unwrap_or_else(|_| payload.payload.clone())
}

/// Execute one reserved payload and finalize it on the driver.
///
/// The job's [`JobPolicy`] (from the registered handler — see
/// [`register_job_with_policy`]) drives retry handling:
///
/// * success/skip `ack`s the reservation;
/// * a retryable failure whose attempt budget remains is `release`d after the
///   policy's exponential backoff delay (`#[backoff(secs)]`);
/// * a failure once `#[tries(n)]` attempts are exhausted is dead-lettered to
///   `failed_jobs` with the attempt's exception trace.
///
/// Timeout enforcement happens inside [`crate::job::run_erased`] using the same
/// policy, so `#[timeout(secs)]` is honoured per attempt.
async fn process_one<F>(driver: &dyn QueueDriver, payload: &JobPayload, resolver: &F) -> Result<()>
where
    F: Fn(&JobPayload) -> Option<Arc<dyn ErasedJob>> + Send + Sync,
{
    let Some(exec) = resolver(payload) else {
        driver
            .dead_letter(FailedJob::new(
                payload.connection.clone(),
                payload.queue.clone(),
                dead_letter_payload(payload),
                "no handler registered for job payload",
            ))
            .await?;
        driver.ack(payload).await?;
        record_batch(payload, true).await;
        return Ok(());
    };

    let policy = exec.policy();
    match run_erased(exec.as_ref()).await {
        JobOutcome::Succeeded | JobOutcome::Skipped => {
            driver.ack(payload).await?;
            release_lease(payload).await;
            record_batch(payload, false).await;
        }
        JobOutcome::Retrying { .. } if policy.allows_retry(payload.attempts) => {
            // The worker owns the global attempt count, so the backoff delay is
            // recomputed from the policy rather than trusting the single-attempt
            // outcome — this keeps exponential growth correct across runs. The
            // unique lease is intentionally kept: the job is still in flight and
            // must not be re-dispatched until it reaches a terminal outcome.
            // No batch update: a retry is not a terminal outcome.
            let delay = policy.delay_for_attempt(payload.attempts);
            driver.release(payload, delay).await?;
        }
        JobOutcome::Failed { exception } | JobOutcome::Retrying { exception, .. } => {
            driver
                .dead_letter(FailedJob::new(
                    payload.connection.clone(),
                    payload.queue.clone(),
                    dead_letter_payload(payload),
                    format!("JobError::MaxAttemptsExceeded: {exception}"),
                ))
                .await?;
            driver.ack(payload).await?;
            release_lease(payload).await;
            record_batch(payload, true).await;
        }
    }
    Ok(())
}

/// Record a terminal outcome against the payload's batch, when it belongs to one.
///
/// Best-effort: a persistence hiccup must not mask the job's already-recorded
/// outcome (ack/dead-letter). Standalone payloads (no `batch_id`) and
/// applications without an installed [`crate::batch_db::DatabaseBatchRepository`]
/// are no-ops. `failed` selects the failure path (decrement + record the id).
async fn record_batch(payload: &JobPayload, failed: bool) {
    let Some(batch_id) = payload.batch_id.as_deref() else {
        return;
    };
    let failed_job_id = if failed {
        let id = payload.id.clone().unwrap_or_else(|| {
            // A payload that reached the batch hook without a driver-assigned
            // reservation id still needs a stable failure id for the
            // `failed_job_ids` array; mint a synthetic one and log it so the
            // origin is diagnosable.
            let synthetic = crate::job::JobId::new().to_string();
            tracing::debug!(
                batch_id = %batch_id,
                queue = %payload.queue,
                connection = %payload.connection,
                synthetic_job_id = %synthetic,
                "minting synthetic job id for batch failure with no payload id"
            );
            synthetic
        });
        Some(id)
    } else {
        None
    };
    let _ = crate::batch_db::record_batch_outcome(batch_id, failed_job_id).await;
}

/// Release the unique lease for a payload that reached a terminal outcome.
///
/// Best-effort: a release failure is swallowed so it never masks the job's
/// already-recorded outcome (ack/dead-letter). Only payloads whose `job` type
/// name resolves to a registered unique type and whose application installed a
/// cache store are affected; everything else is a no-op.
async fn release_lease(payload: &JobPayload) {
    let Some(type_key) = payload.job.as_deref() else {
        return;
    };
    let _ = release_unique(type_key, &payload.payload).await;
}
