/// Per-dispatch builder handle returned by [`crate::job::Job::dispatch`].
///
/// Every override mutates a shared plan; the terminal `await` performs the
/// enqueue through the resolved connection's driver. Split from `job.rs` to keep
/// that module within the file-size standard.
use std::sync::Arc;
use std::time::Duration;

use crate::job::{ErasedJob, JobId};
use crate::registry::Queue;
use crate::unique::{acquire_lease, release_unique, DispatchOutcome, LeaseState};
use crate::Result;

/// Chainable per-dispatch overrides returned by `Job::dispatch`.
#[derive(Clone)]
pub struct DispatchHandle {
    pub(crate) exec: Arc<dyn ErasedJob>,
    pub(crate) queue: Option<String>,
    pub(crate) connection: Option<String>,
    pub(crate) delay: Duration,
    pub(crate) batch_id: Option<String>,
}

impl DispatchHandle {
    /// Create a dispatch handle for an erased job with no overrides yet.
    pub fn new(exec: Arc<dyn ErasedJob>) -> Self {
        Self {
            exec,
            queue: None,
            connection: None,
            delay: Duration::ZERO,
            batch_id: None,
        }
    }

    /// Route this single dispatch to `queue`, overriding the routed queue.
    pub fn on_queue(mut self, queue: impl Into<String>) -> Self {
        self.queue = Some(queue.into());
        self
    }

    /// Route this single dispatch to `connection`, overriding the routed one.
    pub fn on_connection(mut self, connection: impl Into<String>) -> Self {
        self.connection = Some(connection.into());
        self
    }

    /// Delay this single dispatch by `delay` (sets `available_at`).
    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// Attach this dispatch to a batch so its outcome updates the batch counters.
    ///
    /// Sets [`crate::job::JobPayload::batch_id`]; a worker finishing the job then
    /// decrements the batch's `pending_jobs` (see
    /// [`crate::batch_db::record_batch_outcome`]).
    pub fn on_batch(mut self, batch_id: impl Into<String>) -> Self {
        self.batch_id = Some(batch_id.into());
        self
    }

    /// Enqueue the job through the resolved connection and queue.
    pub async fn dispatch(self) -> Result<JobId> {
        crate::registry::Queue::dispatch_handle(self).await
    }
}

/// Resolve the route, honour the unique lease, and enqueue the job.
///
/// This is the single dispatch gate shared by [`Queue::dispatch_handle`] and
/// [`Queue::dispatch_handle_outcome`]. The unique lease is acquired *before* the
/// payload reaches a driver: a [`LeaseState::Held`] result short-circuits to
/// [`DispatchOutcome::Deduplicated`] without enqueuing, matching Laravel's
/// `ShouldBeUnique` semantics. Non-unique jobs and applications that never
/// installed a cache store see [`LeaseState::NotApplicable`] and dispatch
/// exactly as before.
pub(crate) async fn dispatch_outcome(handle: DispatchHandle) -> Result<DispatchOutcome> {
    let type_key = handle.exec.type_key();
    let routed = Queue::resolve(type_key).ok();
    let connection = match (&handle.connection, &routed) {
        (Some(c), _) => c.clone(),
        (None, Some(r)) => r.connection.to_string(),
        (None, None) => return Err(crate::QueueError::Unrouted(type_key.to_string())),
    };
    let queue = match (&handle.queue, &routed) {
        (Some(q), _) => q.clone(),
        (None, Some(r)) => r.queue.to_string(),
        (None, None) => return Err(crate::QueueError::Unrouted(type_key.to_string())),
    };

    let body = handle.exec.as_json();
    let leased = match acquire_lease(type_key, &body).await? {
        LeaseState::Held { unique_id } => {
            return Ok(DispatchOutcome::Deduplicated { unique_id });
        }
        LeaseState::Acquired { .. } => true,
        LeaseState::NotApplicable => false,
    };

    if connection == crate::driver::SYNC_CONNECTION {
        // Inline sync dispatch: do NOT buffer the payload — the queue must stay
        // empty (pending_size == 0) once the job has run. The inline execution
        // is a terminal outcome, so the lease is released here. Best-effort: a
        // release hiccup must not mask the execution outcome.
        let batch_id = handle.batch_id.clone();
        let outcome = Queue::execute_sync(&handle).await;
        if leased {
            let _ = release_unique(type_key, &body).await;
        }
        // A sync batch job is terminal here (no later worker), so its outcome is
        // recorded against the batch immediately. Best-effort: a persistence
        // failure must not mask the job's execution outcome.
        if let Some(batch_id) = batch_id {
            let failed = sync_batch_failure_id(&outcome);
            let _ = crate::batch_db::record_batch_outcome(&batch_id, failed).await;
        }
        return outcome.map(|_| DispatchOutcome::Enqueued(JobId::new()));
    }

    let payload = crate::registry::to_payload(
        type_key,
        body.clone(),
        queue,
        connection.clone(),
        handle.delay,
        handle.batch_id.clone(),
    )?;
    if let Err(e) = crate::registry::driver(&connection)?.push(payload).await {
        // Enqueue failed: release the lease so the job is not wedged until its
        // TTL — a retry should be allowed immediately.
        if leased {
            let _ = release_unique(type_key, &body).await;
        }
        return Err(e);
    }
    Ok(DispatchOutcome::Enqueued(JobId::new()))
}

/// Map a sync execution result to the batch-failure marker for the outcome hook.
///
/// `None` records a clean success (`pending_jobs` decrement); `Some(id)` records
/// a failure. Both a non-success [`JobOutcome`] *and* an execution `Err` are
/// failures — a sync job that errored must never be counted as a batch success.
fn sync_batch_failure_id(outcome: &Result<crate::job::JobOutcome>) -> Option<String> {
    match outcome {
        Ok(crate::job::JobOutcome::Succeeded) | Ok(crate::job::JobOutcome::Skipped) => None,
        Ok(crate::job::JobOutcome::Failed { .. }) | Ok(crate::job::JobOutcome::Retrying { .. }) => {
            Some(JobId::new().to_string())
        }
        Err(_) => Some(JobId::new().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clean sync success maps to a non-failure (no id recorded).
    #[test]
    fn sync_success_maps_to_no_failure() {
        assert!(sync_batch_failure_id(&Ok(crate::job::JobOutcome::Succeeded)).is_none());
        assert!(sync_batch_failure_id(&Ok(crate::job::JobOutcome::Skipped)).is_none());
    }

    /// A failed/retrying sync outcome maps to a failure id.
    #[test]
    fn sync_failed_outcome_maps_to_failure() {
        let failed = crate::job::JobOutcome::Failed {
            exception: "boom".to_string(),
        };
        assert!(sync_batch_failure_id(&Ok(failed)).is_some());
        let retrying = crate::job::JobOutcome::Retrying {
            attempt: 2,
            delay: std::time::Duration::ZERO,
            exception: "boom".to_string(),
        };
        assert!(sync_batch_failure_id(&Ok(retrying)).is_some());
    }

    /// A sync execution `Err` maps to a failure, never a clean success.
    #[test]
    fn sync_execution_error_maps_to_failure() {
        let error: Result<crate::job::JobOutcome> = Err(
            crate::error::QueueError::StoreUnavailable("down".to_string()),
        );
        assert!(
            sync_batch_failure_id(&error).is_some(),
            "an execution error must record a batch failure, not a success"
        );
    }
}
