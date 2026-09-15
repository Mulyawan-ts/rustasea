//! Crate-level integration tests — the three real hooks plus install/uninstall.
//!
//! These exercise the seams end-to-end: the ORM SQL hook, the events observer,
//! and the cache store decorator, each against an installed
//! [`crate::ProfileContext`]. The process-wide slots are shared across the whole
//! test binary, so every test takes the crate-wide serialization lock.

use std::sync::Arc;

use rustasea_cache::{MemoryStore, Store};

use crate::context::{self, CacheEntry, ProfileContext};
use crate::test_support::PROFILER_LOCK;
use crate::{clear_ring, install, record, set_capacity, snapshot, uninstall, DEFAULT_CAPACITY};

/// Verifies `install` registers both hooks and `uninstall` clears them.
#[tokio::test]
async fn install_and_uninstall_round_trip() {
    let _guard = PROFILER_LOCK.lock().await;
    uninstall();
    assert!(rustasea_orm::profile::query_recorder().is_none());
    assert!(rustasea_events::dispatch_observer().is_none());

    install(DEFAULT_CAPACITY);
    assert!(rustasea_orm::profile::query_recorder().is_some());
    assert!(rustasea_events::dispatch_observer().is_some());

    uninstall();
    assert!(rustasea_orm::profile::query_recorder().is_none());
    assert!(rustasea_events::dispatch_observer().is_none());
    assert!(snapshot().is_empty());
}

/// Verifies the ring capacity is clamped to at least one entry.
#[tokio::test]
async fn ring_capacity_is_at_least_one() {
    let _guard = PROFILER_LOCK.lock().await;
    set_capacity(0);
    record(crate::context::RequestProfile {
        method: "GET".to_string(),
        path: "/x".to_string(),
        request_id: None,
        status: 200,
        duration_ms: 0.0,
        queries: Vec::new(),
        cache_ops: Vec::new(),
        events: Vec::new(),
        started_at: "2026-01-01T00:00:00Z".to_string(),
    });
    assert_eq!(snapshot().len(), 1);
    clear_ring();
}

/// Build and install a fresh context, returning it for assertions.
fn active_context() -> Arc<ProfileContext> {
    let ctx = Arc::new(ProfileContext::new("GET", "/test", None));
    context::set_current(Arc::clone(&ctx));
    ctx
}

/// Verifies the installed ORM hook records SQL through `profile::track`.
#[tokio::test]
async fn orm_hook_records_sql_when_context_active() {
    let _guard = PROFILER_LOCK.lock().await;
    uninstall();
    install(DEFAULT_CAPACITY);
    let ctx = active_context();

    let value = rustasea_orm::profile::track("fetch_json", "SELECT 1", async {
        Ok::<_, rustasea_orm::OrmError>(42)
    })
    .await
    .unwrap();
    assert_eq!(value, 42);

    let (queries, _, _) = ctx.drain();
    assert_eq!(queries.len(), 1);
    assert_eq!(queries[0].sql, "SELECT 1");
    assert!(queries[0].error.is_none());

    context::clear_current_if(&ctx);
    uninstall();
}

/// Verifies the ORM hook records nothing when no request context is active.
#[tokio::test]
async fn orm_hook_skips_without_context() {
    let _guard = PROFILER_LOCK.lock().await;
    uninstall();
    install(DEFAULT_CAPACITY);
    // No `set_current`: the hook must drop the event silently.
    let value = rustasea_orm::profile::track("execute_bind", "DELETE FROM t", async {
        Ok::<_, rustasea_orm::OrmError>(1)
    })
    .await
    .unwrap();
    assert_eq!(value, 1);
    assert!(context::current().is_none());
    uninstall();
}

/// Verifies the installed events observer records a dispatched event.
#[tokio::test]
async fn events_hook_records_dispatch() {
    let _guard = PROFILER_LOCK.lock().await;
    uninstall();
    install(DEFAULT_CAPACITY);
    let ctx = active_context();

    rustasea_events::Dispatcher::listen::<DebugbarTestEvent, _>(DebugbarTestListener);
    rustasea_events::Dispatcher::dispatch(DebugbarTestEvent)
        .await
        .unwrap();

    let (_, _, events) = ctx.drain();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].name, "DebugbarTestEvent");
    assert!(events[0].ok);

    context::clear_current_if(&ctx);
    uninstall();
}

/// Event unique to this test module so the shared registry cannot collide.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct DebugbarTestEvent;

impl rustasea_events::Event for DebugbarTestEvent {
    fn event_name(&self) -> &'static str {
        "DebugbarTestEvent"
    }
}

/// Listener making `DebugbarTestEvent` dispatchable.
struct DebugbarTestListener;

#[rustasea_events::async_trait]
impl rustasea_events::Listener<DebugbarTestEvent> for DebugbarTestListener {
    async fn handle(&self, _event: DebugbarTestEvent) -> rustasea_events::Result<()> {
        Ok(())
    }
}

/// Verifies the cache decorator forwards and records a `get` hit and a `put`.
#[tokio::test]
async fn cache_wrapper_forwards_and_records() {
    let _guard = PROFILER_LOCK.lock().await;
    uninstall();
    install(DEFAULT_CAPACITY);
    let ctx = active_context();

    let store = crate::recorders::wrap(Arc::new(MemoryStore::new()) as Arc<dyn Store>);
    store
        .put("k", b"v".to_vec(), std::time::Duration::ZERO)
        .await
        .unwrap();
    let hit = store.get("k").await.unwrap();
    assert_eq!(hit.as_deref(), Some(b"v".as_slice()));

    let (_, cache_ops, _) = ctx.drain();
    assert_eq!(cache_ops.len(), 2);
    assert_eq!(cache_ops[0].op, "put");
    assert_eq!(cache_ops[0].key, "k");
    assert!(cache_ops[0].hit.is_none());
    assert_eq!(cache_ops[1].op, "get");
    assert_eq!(cache_ops[1].hit, Some(true));

    context::clear_current_if(&ctx);
    uninstall();
}

/// Verifies the cache decorator forwards a miss and records `hit = Some(false)`.
#[tokio::test]
async fn cache_wrapper_records_a_miss() {
    let _guard = PROFILER_LOCK.lock().await;
    uninstall();
    install(DEFAULT_CAPACITY);
    let ctx = active_context();

    let store = crate::recorders::wrap(Arc::new(MemoryStore::new()) as Arc<dyn Store>);
    assert!(store.get("absent").await.unwrap().is_none());

    let (_, cache_ops, _) = ctx.drain();
    assert_eq!(cache_ops.len(), 1);
    assert_eq!(cache_ops[0].hit, Some(false));

    context::clear_current_if(&ctx);
    uninstall();
}

/// A `CacheEntry` round-trips through JSON for the `/_debugbar/json` payload.
#[test]
fn cache_entry_serializes() {
    let entry = CacheEntry {
        op: "get".to_string(),
        key: "k".to_string(),
        hit: Some(true),
        duration_ms: 0.25,
    };
    let json = serde_json::to_string(&entry).unwrap();
    assert!(json.contains("\"hit\":true"));
}
