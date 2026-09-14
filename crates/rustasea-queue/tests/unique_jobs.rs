//! `ShouldBeUnique` unique-job locking (LARAVEL-012).
//!
//! Positive: a second identical dispatch while the first is in flight is
//! deduplicated (the lease is held). Negative: once the first job reaches a
//! terminal outcome the lease is released, so an identical dispatch succeeds
//! again. Also covers payload-sensitive default ids and the [`UniqueGuard`] RAII
//! helper. The store is a single process-wide [`MemoryStore`] shared by every
//! test, so isolation is achieved structurally: each test owns a *distinct job
//! type, connection, and queue*, keeping the process-wide route registry, the
//! unique-key namespace, and the per-connection driver buffers disjoint even
//! when the harness runs tests in parallel.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use rustasea_cache::{MemoryStore, Store};
use rustasea_queue::{
    register_unique, set_unique_store, DispatchOutcome, Job, JobError, Queue, ShouldBeUnique,
    UniqueGuard,
};
use serde::{Deserialize, Serialize};

/// Process-wide cache store backing every unique test.
static STORE: OnceLock<Arc<MemoryStore>> = OnceLock::new();

/// Install (once) and return the shared in-memory unique store.
fn store() -> Arc<MemoryStore> {
    STORE
        .get_or_init(|| {
            let store = Arc::new(MemoryStore::new());
            set_unique_store(store.clone() as Arc<dyn Store>);
            store
        })
        .clone()
}

/// Register a buffered (non-sync) connection + route for `J`.
///
/// The `connection` must be unique per test: the route registry is keyed by
/// `std::any::type_name::<J>()` (so a type may be routed only once per process),
/// and `SyncDriver::pending_size` reports its whole buffer regardless of queue
/// name — sharing either would leak state across tests under parallel execution.
fn route_buffered<J: Job + 'static>(connection: &'static str, queue: &'static str) {
    Queue::register_driver(connection, Arc::new(rustasea_queue::SyncDriver::new()));
    Queue::route::<J>(connection, queue).expect("route registered once");
}

/// Declare a `ShouldBeUnique` job with a single `source` field and an empty body.
///
/// Each test instantiates its own type so the process-wide route registry never
/// sees two registrations for the same `type_name` — the source of the original
/// `DuplicateRoute` panic when the harness runs tests in the same process.
macro_rules! unique_buffered_job {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Serialize, Deserialize)]
        struct $name {
            /// Domain key identifying the import batch.
            source: String,
        }

        #[async_trait::async_trait]
        impl Job for $name {
            /// No-op body; this test exercises the dispatch gate, not execution.
            async fn handle(self) -> std::result::Result<(), JobError> {
                Ok(())
            }
        }

        impl ShouldBeUnique for $name {}
    };
}

unique_buffered_job!(
    /// Unique job whose two instances with the same payload must deduplicate.
    UniqueImport
);

unique_buffered_job!(
    /// Second unique job type so the distinct-payload test owns its own route.
    UniqueImportDistinct
);

/// Unique job that runs inline on the `sync` connection for the release test.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct UniqueSyncJob {
    /// Domain key identifying the unit of work.
    id: u32,
}

#[async_trait::async_trait]
impl Job for UniqueSyncJob {
    /// Always succeeds so the sync dispatch reaches a terminal outcome.
    async fn handle(self) -> std::result::Result<(), JobError> {
        Ok(())
    }
}

impl ShouldBeUnique for UniqueSyncJob {
    /// Narrow the uniqueness scope to the domain id.
    fn unique_id(&self) -> String {
        format!("UniqueSyncJob:{}", self.id)
    }

    /// Short lease so the test does not depend on the default hour.
    fn unique_for(&self) -> Duration {
        Duration::from_secs(60)
    }
}

/// Positive: a second identical dispatch is deduplicated while in flight.
#[tokio::test]
async fn second_identical_dispatch_is_deduplicated() {
    let _ = store();
    register_unique::<UniqueImport>();
    route_buffered::<UniqueImport>("uq-positive-conn", "uq-positive");

    let first = Queue::dispatch_handle_outcome(UniqueImport { source: "a".into() }.dispatch())
        .await
        .expect("first dispatch");
    assert!(matches!(first, DispatchOutcome::Enqueued(_)));

    let second = Queue::dispatch_handle_outcome(UniqueImport { source: "a".into() }.dispatch())
        .await
        .expect("second dispatch");
    assert!(
        matches!(second, DispatchOutcome::Deduplicated { .. }),
        "identical in-flight dispatch must be deduplicated, got {second:?}"
    );

    // Exactly one payload reached the buffered driver.
    assert_eq!(
        Queue::pending_size("uq-positive-conn", "uq-positive")
            .await
            .expect("pending size"),
        1
    );
}

/// Negative: a dispatch after the first reached a terminal outcome succeeds.
#[tokio::test]
async fn dispatch_after_completion_succeeds() {
    let _ = store();
    register_unique::<UniqueSyncJob>();
    Queue::route::<UniqueSyncJob>("sync", "uq-negative").expect("route");

    let first = Queue::dispatch_handle_outcome(UniqueSyncJob { id: 7 }.dispatch())
        .await
        .expect("first sync dispatch");
    assert!(matches!(first, DispatchOutcome::Enqueued(_)));

    // The sync run is terminal, so the lease was released during dispatch.
    let second = Queue::dispatch_handle_outcome(UniqueSyncJob { id: 7 }.dispatch())
        .await
        .expect("second sync dispatch");
    assert!(
        matches!(second, DispatchOutcome::Enqueued(_)),
        "a completed unique job must be dispatchable again, got {second:?}"
    );
}

/// Different payloads derive different default ids and both enqueue.
#[tokio::test]
async fn distinct_payloads_are_not_deduplicated() {
    let _ = store();
    register_unique::<UniqueImportDistinct>();
    route_buffered::<UniqueImportDistinct>("uq-distinct-conn", "uq-distinct");

    let a = Queue::dispatch_handle_outcome(UniqueImportDistinct { source: "x".into() }.dispatch())
        .await
        .expect("dispatch a");
    let b = Queue::dispatch_handle_outcome(UniqueImportDistinct { source: "y".into() }.dispatch())
        .await
        .expect("dispatch b");
    assert!(matches!(a, DispatchOutcome::Enqueued(_)));
    assert!(
        matches!(b, DispatchOutcome::Enqueued(_)),
        "different payloads must not collide, got {b:?}"
    );
}

/// The default `unique_id` is stable for equal payloads and differs otherwise.
#[test]
fn default_unique_id_tracks_payload() {
    let a = UniqueImport {
        source: "same".into(),
    };
    let b = UniqueImport {
        source: "same".into(),
    };
    let c = UniqueImport {
        source: "other".into(),
    };
    assert_eq!(a.unique_id(), b.unique_id());
    assert_ne!(a.unique_id(), c.unique_id());
    // Default `unique_for` normalises to the sane default at acquisition.
    assert_eq!(a.unique_for(), Duration::ZERO);
}

/// `UniqueGuard` excludes a second holder until the first releases.
#[tokio::test]
async fn unique_guard_excludes_until_released() {
    let store = store();
    let mut first = UniqueGuard::acquire(store.clone(), "guard-test", Duration::from_secs(30))
        .await
        .expect("acquire")
        .expect("first holder");
    assert_eq!(first.id(), "guard-test");

    let blocked = UniqueGuard::acquire(store.clone(), "guard-test", Duration::from_secs(30))
        .await
        .expect("second acquire");
    assert!(blocked.is_none(), "second holder must be excluded");

    first.release().await.expect("release");
    let after = UniqueGuard::acquire(store.clone(), "guard-test", Duration::from_secs(30))
        .await
        .expect("acquire after release");
    assert!(after.is_some(), "lease must be re-acquirable after release");
}
