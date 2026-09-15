//! Profiler hook implementations for the SQL, event, and cache seams.
//!
//! Three thin decorators connect the framework's instrumentation seams to the
//! current [`ProfileContext`]:
//!
//! * [`OrmQueryRecorder`] implements `rustasea_orm::profile::QueryRecorder`.
//! * [`EventsObserver`] implements `rustasea_events::DispatchObserver`.
//! * [`RecordingStore`] implements `rustasea_cache::Store`, forwarding every
//!   operation to the real store while recording it.
//!
//! Each hook is a no-op when no request context is active ([`context::current`]
//! returns `None`), so work outside a profiled request costs only a slot read.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;

use rustasea_cache::{CacheManager, Result as CacheResult, Store, StoreRef};
use rustasea_events::DispatchObserver;
use rustasea_orm::profile::{QueryRecorder, SqlQueryEvent};

use crate::context::{self, CacheEntry, EventEntry, SqlEntry};

/// SQL recorder — pushes each measured call onto the current context.
pub struct OrmQueryRecorder;

impl QueryRecorder for OrmQueryRecorder {
    /// Record one measured SQL call, or drop it when no request is active.
    fn record(&self, event: SqlQueryEvent) {
        if let Some(ctx) = context::current() {
            ctx.push_query(SqlEntry {
                sql: event.sql,
                duration_ms: event.duration.as_secs_f64() * 1000.0,
                error: event.error,
            });
        }
    }
}

/// Event observer — pushes each dispatched event onto the current context.
pub struct EventsObserver;

impl DispatchObserver for EventsObserver {
    /// Record one dispatched event, or drop it when no request is active.
    fn observe(&self, event_name: &'static str, ok: bool) {
        if let Some(ctx) = context::current() {
            ctx.push_event(EventEntry {
                name: event_name.to_string(),
                ok,
            });
        }
    }
}

/// Cache decorator — forwards every operation to `inner`, recording it.
///
/// The shape mirrors `rustasea_testing::FakeCache`'s forward-and-record store:
/// `get` records the hit flag (`Some(result.is_some())`), while the mutating
/// operations record `None` for the hit column. Every operation is timed.
pub struct RecordingStore {
    /// The real store every operation is forwarded to.
    inner: StoreRef,
}

impl RecordingStore {
    /// Wrap `inner` in a recording decorator.
    pub fn new(inner: StoreRef) -> Self {
        Self { inner }
    }

    /// Record one operation against the current context (skipped when idle).
    fn record(&self, op: &str, key: &str, hit: Option<bool>, started: Instant) {
        if let Some(ctx) = context::current() {
            ctx.push_cache(CacheEntry {
                op: op.to_string(),
                key: key.to_string(),
                hit,
                duration_ms: started.elapsed().as_secs_f64() * 1000.0,
            });
        }
    }
}

#[async_trait]
impl Store for RecordingStore {
    /// Record the `get` hit/miss and forward to the real store.
    async fn get(&self, key: &str) -> CacheResult<Option<Vec<u8>>> {
        let started = Instant::now();
        let result = self.inner.get(key).await;
        self.record(
            "get",
            key,
            Some(result.as_ref().is_ok_and(Option::is_some)),
            started,
        );
        result
    }

    /// Record the `put` and forward to the real store.
    async fn put(&self, key: &str, value: Vec<u8>, ttl: Duration) -> CacheResult<()> {
        let started = Instant::now();
        let result = self.inner.put(key, value, ttl).await;
        self.record("put", key, None, started);
        result
    }

    /// Record the `put_if_absent` and forward to the real store.
    async fn put_if_absent(&self, key: &str, value: Vec<u8>, ttl: Duration) -> CacheResult<bool> {
        let started = Instant::now();
        let result = self.inner.put_if_absent(key, value, ttl).await;
        self.record("put_if_absent", key, None, started);
        result
    }

    /// Record the `compare_and_delete` and forward to the real store.
    async fn compare_and_delete(&self, key: &str, expected: &[u8]) -> CacheResult<bool> {
        let started = Instant::now();
        let result = self.inner.compare_and_delete(key, expected).await;
        self.record("compare_and_delete", key, None, started);
        result
    }

    /// Record the `touch` and forward to the real store.
    async fn touch(&self, key: &str, ttl: Duration) -> CacheResult<bool> {
        let started = Instant::now();
        let result = self.inner.touch(key, ttl).await;
        self.record("touch", key, None, started);
        result
    }

    /// Record the `forget` and forward to the real store.
    async fn forget(&self, key: &str) -> CacheResult<()> {
        let started = Instant::now();
        let result = self.inner.forget(key).await;
        self.record("forget", key, None, started);
        result
    }

    /// Record the `flush` and forward to the real store.
    async fn flush(&self) -> CacheResult<()> {
        let started = Instant::now();
        let result = self.inner.flush().await;
        self.record("flush", "", None, started);
        result
    }

    /// Record the `increment` and forward to the real store.
    ///
    /// `decrement` uses the trait default, which routes here and is therefore
    /// recorded as an `increment` with a negative amount too.
    async fn increment(&self, key: &str, n: i64) -> CacheResult<i64> {
        let started = Instant::now();
        let result = self.inner.increment(key, n).await;
        self.record("increment", key, None, started);
        result
    }
}

/// Wrap `store` in a [`RecordingStore`], returning the shared handle.
pub fn wrap(store: StoreRef) -> StoreRef {
    Arc::new(RecordingStore::new(store))
}

/// Re-register every store in `manager` wrapped in a [`RecordingStore`].
///
/// Enumerates the manager's registry via [`CacheManager::store_names`] and, for
/// each name, resolves the current store through [`CacheManager::store`] and
/// re-registers a recording decorator over it. Names are sorted, so the
/// re-registration order is deterministic. A name that resolves to an error
/// (registry poisoned or racing) is skipped rather than aborting the wrap.
///
/// Idempotency is the caller's concern: calling this twice nests decorators.
/// [`crate::install`] guards against a repeat install.
pub fn wrap_cache_manager(manager: &CacheManager) {
    for name in manager.store_names() {
        let Ok(repository) = manager.store(&name) else {
            continue;
        };
        let inner = Arc::clone(repository.inner());
        manager.register(name, wrap(inner));
    }
}
