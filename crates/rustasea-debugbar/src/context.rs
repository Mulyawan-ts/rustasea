//! Per-request profiling context and the serializable profile record.
//!
//! A [`ProfileContext`] is created by the profiler middleware at request entry
//! and installed in a process-wide slot ([`set_current`]) so the SQL, cache, and
//! event hooks can append to it without threading a handle through every call.
//! At request exit the middleware drains the context into a [`RequestProfile`]
//! and clears the slot.
//!
//! # Known limitation — process-wide current-context slot
//!
//! The slot holds one `Arc<ProfileContext>`, so concurrent requests in a
//! multi-threaded dev server can interleave: a hook running on request B's task
//! may observe request A's context if A is still in flight. This is the same
//! accepted trade-off as the activity-log causer slot (`rustasea_orm::activity`)
//! — the toolbar is a dev-only aid, and the debug server is normally exercised
//! one request at a time. Production never installs the profiler at all.

use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Instant;

use serde::Serialize;

/// One recorded SQL call.
#[derive(Debug, Clone, Serialize)]
pub struct SqlEntry {
    /// The original SQL text (before placeholder adaptation).
    pub sql: String,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: f64,
    /// Driver error rendered as a string, or `None` on success.
    pub error: Option<String>,
}

/// One recorded cache operation.
#[derive(Debug, Clone, Serialize)]
pub struct CacheEntry {
    /// Operation kind (`get`, `put`, `forget`, …).
    pub op: String,
    /// The key the operation targeted (empty for `flush`).
    pub key: String,
    /// `Some(true)`/`Some(false)` for a `get` hit/miss; `None` for writes.
    pub hit: Option<bool>,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: f64,
}

/// One recorded dispatched event.
#[derive(Debug, Clone, Serialize)]
pub struct EventEntry {
    /// The event's stable name.
    pub name: String,
    /// Whether dispatch completed without error.
    pub ok: bool,
}

/// Accumulating, request-scoped profiling state.
///
/// Each collection is behind its own poison-tolerant [`Mutex`]; the hooks push
/// without ever surfacing a panic from another thread. The middleware drains
/// every collection into a [`RequestProfile`] when the response is ready.
pub struct ProfileContext {
    /// HTTP method of the request.
    pub method: String,
    /// Request path.
    pub path: String,
    /// Value of the `x-request-id` header, if present.
    pub request_id: Option<String>,
    /// Instant the request entered the profiler.
    pub started: Instant,
    /// Recorded SQL calls, in call order.
    pub queries: Mutex<Vec<SqlEntry>>,
    /// Recorded cache operations, in call order.
    pub cache_ops: Mutex<Vec<CacheEntry>>,
    /// Recorded dispatched events, in call order.
    pub events: Mutex<Vec<EventEntry>>,
}

impl ProfileContext {
    /// Create a context for a request entering the profiler.
    pub fn new(
        method: impl Into<String>,
        path: impl Into<String>,
        request_id: Option<String>,
    ) -> Self {
        Self {
            method: method.into(),
            path: path.into(),
            request_id,
            started: Instant::now(),
            queries: Mutex::new(Vec::new()),
            cache_ops: Mutex::new(Vec::new()),
            events: Mutex::new(Vec::new()),
        }
    }

    /// Append a SQL entry (poison-tolerant).
    pub fn push_query(&self, entry: SqlEntry) {
        self.queries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(entry);
    }

    /// Append a cache entry (poison-tolerant).
    pub fn push_cache(&self, entry: CacheEntry) {
        self.cache_ops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(entry);
    }

    /// Append an event entry (poison-tolerant).
    pub fn push_event(&self, entry: EventEntry) {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(entry);
    }

    /// Drain the accumulated collections into an owned snapshot.
    ///
    /// Called by the middleware once the response is ready; the context is
    /// cleared from the slot immediately afterwards.
    pub fn drain(&self) -> (Vec<SqlEntry>, Vec<CacheEntry>, Vec<EventEntry>) {
        let queries = std::mem::take(
            &mut *self
                .queries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        let cache_ops = std::mem::take(
            &mut *self
                .cache_ops
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        let events = std::mem::take(
            &mut *self
                .events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        (queries, cache_ops, events)
    }
}

/// A finalized, serializable profile for one request.
#[derive(Debug, Clone, Serialize)]
pub struct RequestProfile {
    /// HTTP method.
    pub method: String,
    /// Request path.
    pub path: String,
    /// Value of the `x-request-id` header, if present.
    pub request_id: Option<String>,
    /// Response status code.
    pub status: u16,
    /// Total request duration in milliseconds.
    pub duration_ms: f64,
    /// Recorded SQL calls.
    pub queries: Vec<SqlEntry>,
    /// Recorded cache operations.
    pub cache_ops: Vec<CacheEntry>,
    /// Recorded dispatched events.
    pub events: Vec<EventEntry>,
    /// RFC 3339 UTC instant the request entered the profiler.
    pub started_at: String,
}

/// Process-wide current-context slot.
static CURRENT: OnceLock<RwLock<Option<Arc<ProfileContext>>>> = OnceLock::new();

/// The current-context slot, initialised to empty on first use.
fn current_slot() -> &'static RwLock<Option<Arc<ProfileContext>>> {
    CURRENT.get_or_init(|| RwLock::new(None))
}

/// Install `context` as the current request's profiling context.
pub fn set_current(context: Arc<ProfileContext>) {
    let slot = current_slot();
    let mut guard = slot
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *guard = Some(context);
}

/// The current request's profiling context, or `None` when no request is active.
///
/// The SQL/cache/event hooks call this and skip recording when it returns `None`,
/// so work done outside a profiled request costs only a slot read.
pub fn current() -> Option<Arc<ProfileContext>> {
    let slot = current_slot();
    let guard = slot
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.clone()
}

/// Clear the current context only when it is still `same`.
///
/// Compares by pointer identity ([`Arc::ptr_eq`]) before clearing, so a request
/// that finishes late cannot clobber a newer request's context that has already
/// replaced it in the slot.
pub fn clear_current_if(same: &Arc<ProfileContext>) {
    let slot = current_slot();
    let mut guard = slot
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let matches = guard
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, same));
    if matches {
        *guard = None;
    }
}
