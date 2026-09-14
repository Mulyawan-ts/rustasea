/// Per-dispatch builder handle returned by [`crate::job::Job::dispatch`].
///
/// Every override mutates a shared plan; the terminal `await` performs the
/// enqueue through the resolved connection's driver. Split from `job.rs` to keep
/// that module within the file-size standard.
use std::sync::Arc;
use std::time::Duration;

use crate::job::{ErasedJob, JobId};
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
