//! `FakeQueue` — the `Queue::fake()` equivalent.
//!
//! [`FakeQueue`] implements [`rustasea_queue::QueueDriver`], so it drops straight
//! into the real [`rustasea_queue::Queue::register_driver`] seam: register it
//! under a connection (or use [`FakeQueue::install`]), route jobs there, and
//! every `push` is recorded instead of persisted. No worker, database, or Redis
//! is involved.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use rustasea_queue::{Job, JobPayload, Queue, QueueDriver, Result as QueueResult};

/// Recording queue driver — the `Queue::fake()` equivalent.
///
/// Implements [`QueueDriver`] so it drops straight into the real
/// [`rustasea_queue::Queue::register_driver`] seam: register it under a
/// connection (or use [`FakeQueue::install`]), route jobs there, and every
/// `push` is recorded instead of persisted. No worker, database, or Redis is
/// involved.
///
/// The recorded payloads are inspected through
/// [`assert_pushed`](FakeQueue::assert_pushed) and friends.
///
/// ```no_run
/// use rustasea_testing::FakeQueue;
/// # use rustasea_queue::{Job, Queue, JobError};
/// # #[derive(serde::Serialize)] struct SendReceipt { id: u64 }
/// # #[rustasea_queue::async_trait]
/// # impl Job for SendReceipt { async fn handle(self) -> Result<(), JobError> { Ok(()) } }
/// # async fn run() -> rustasea_queue::Result<()> {
/// let queue = FakeQueue::install("fake");
/// Queue::route::<SendReceipt>("fake", "default")?;
/// Queue::dispatch(SendReceipt { id: 1 }).await?;
/// queue.assert_pushed::<SendReceipt>();
/// queue.assert_pushed_on::<SendReceipt>("default");
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Default)]
pub struct FakeQueue {
    /// Serialized payloads intercepted at `push`, in enqueue order.
    pushed: Arc<Mutex<Vec<JobPayload>>>,
}

impl FakeQueue {
    /// Create an empty recording queue (not yet registered).
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a recording queue and register it under `connection`.
    ///
    /// Returns the shared handle so the test can assert against the same
    /// instance the real dispatcher pushes to.
    pub fn install(connection: impl Into<String>) -> Arc<Self> {
        let fake = Arc::new(Self::default());
        Queue::register_driver(connection, fake.clone());
        fake
    }

    /// Snapshot of every intercepted payload, in enqueue order.
    pub fn pushed(&self) -> Vec<JobPayload> {
        self.pushed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Number of intercepted payloads.
    pub fn count(&self) -> usize {
        self.pushed.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    /// Intercepted payloads whose type key matches `J`, in enqueue order.
    pub fn pushed_of<J: Job>(&self) -> Vec<JobPayload> {
        let key = std::any::type_name::<J>();
        self.pushed()
            .into_iter()
            .filter(|p| p.job.as_deref() == Some(key))
            .collect()
    }

    /// Assert a job of type `J` was pushed.
    ///
    /// Matching is by the job's registry key, which defaults to
    /// `std::any::type_name::<J>()` (the value [`rustasea_queue::Job::queue_name`]
    /// produces). A job that overrides `queue_name` should be asserted through
    /// [`FakeQueue::pushed`] instead.
    ///
    /// # Panics
    ///
    /// Panics with a message listing the recorded `job@queue` entries when no
    /// matching push was recorded.
    pub fn assert_pushed<J: Job>(&self) {
        if self.pushed_of::<J>().is_empty() {
            panic!(
                "expected job `{}` to have been pushed, but it was not.\nrecorded pushes: [{}]",
                std::any::type_name::<J>(),
                self.recorded_summary()
            );
        }
    }

    /// Assert a job of type `J` was **not** pushed.
    ///
    /// # Panics
    ///
    /// Panics naming the push count when a matching push was recorded.
    pub fn assert_not_pushed<J: Job>(&self) {
        let times = self.pushed_of::<J>().len();
        if times > 0 {
            panic!(
                "expected job `{}` NOT to have been pushed, but it was pushed {times} time(s).",
                std::any::type_name::<J>()
            );
        }
    }

    /// Assert a job of type `J` was pushed exactly `times` times.
    ///
    /// # Panics
    ///
    /// Panics when the recorded count differs from `times`.
    pub fn assert_pushed_times<J: Job>(&self, times: usize) {
        let actual = self.pushed_of::<J>().len();
        assert!(
            actual == times,
            "expected job `{}` to have been pushed {times} time(s), but it was pushed {actual} time(s).\nrecorded pushes: [{}]",
            std::any::type_name::<J>(),
            self.recorded_summary()
        );
    }

    /// Assert a job of type `J` was pushed onto `queue`.
    ///
    /// # Panics
    ///
    /// Panics with a message listing the recorded `job@queue` entries when no
    /// matching push was recorded.
    pub fn assert_pushed_on<J: Job>(&self, queue: &str) {
        let key = std::any::type_name::<J>();
        let matched = self
            .pushed()
            .iter()
            .any(|p| p.job.as_deref() == Some(key) && p.queue == queue);
        if !matched {
            panic!(
                "expected job `{key}` to have been pushed onto queue `{queue}`, but it was not.\nrecorded pushes: [{}]",
                self.recorded_summary()
            );
        }
    }

    /// Assert nothing at all was pushed.
    ///
    /// # Panics
    ///
    /// Panics listing the recorded pushes when any push occurred.
    pub fn assert_nothing_pushed(&self) {
        let count = self.count();
        assert!(
            count == 0,
            "expected no jobs to have been pushed, but {count} were.\nrecorded pushes: [{}]",
            self.recorded_summary()
        );
    }

    /// Drop every recorded push.
    pub fn clear(&self) {
        self.pushed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }

    /// Comma-separated list of recorded `job@queue` entries, for failures.
    fn recorded_summary(&self) -> String {
        let guard = self.pushed.lock().unwrap_or_else(|p| p.into_inner());
        guard
            .iter()
            .map(|p| format!("{}@{}", p.job.as_deref().unwrap_or("?"), p.queue))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[async_trait]
impl QueueDriver for FakeQueue {
    /// Record the payload instead of persisting it.
    async fn push(&self, payload: JobPayload) -> QueueResult<()> {
        self.pushed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(payload);
        Ok(())
    }

    /// Never hand a job to a worker — a fake never executes.
    async fn pop(&self, _queue: &str, _timeout: Duration) -> QueueResult<Option<JobPayload>> {
        Ok(None)
    }

    /// Report the number of intercepted pushes.
    async fn pending_size(&self, _queue: &str) -> QueueResult<usize> {
        Ok(self.count())
    }

    /// A fake never delays jobs.
    async fn delayed_size(&self, _queue: &str) -> QueueResult<usize> {
        Ok(0)
    }

    /// A fake reserves nothing.
    async fn reserved_size(&self, _queue: &str) -> QueueResult<usize> {
        Ok(0)
    }

    /// A fake has no oldest pending job.
    async fn creation_time_of_oldest_pending_job(
        &self,
        _queue: &str,
    ) -> QueueResult<Option<chrono::DateTime<chrono::Utc>>> {
        Ok(None)
    }
}
