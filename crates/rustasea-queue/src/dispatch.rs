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
}

impl DispatchHandle {
    /// Create a dispatch handle for an erased job with no overrides yet.
    pub fn new(exec: Arc<dyn ErasedJob>) -> Self {
        Self {
            exec,
            queue: None,
            connection: None,
            delay: Duration::ZERO,
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
        let outcome = Queue::execute_sync(&handle).await;
        if leased {
            let _ = release_unique(type_key, &body).await;
        }
        return outcome.map(|_| DispatchOutcome::Enqueued(JobId::new()));
    }

    let payload = crate::registry::to_payload(
        type_key,
        body.clone(),
        queue,
        connection.clone(),
        handle.delay,
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
