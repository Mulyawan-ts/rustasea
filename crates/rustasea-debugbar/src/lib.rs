//! RustaSea Debugbar — a dev-only request profiler / debug toolbar (ADOPT-009).
//!
//! Parity target: `barryvdh/laravel-debugbar` + `itsgoingd/clockwork`. The crate
//! captures, per request, the SQL calls, cache operations, and dispatched events
//! that occurred while the request ran, stores the last [`DEFAULT_CAPACITY`]
//! requests in an in-memory ring, and exposes them for the `/_debugbar` UI and
//! its `/_debugbar/json` data endpoint.
//!
//! # Process-wide hooks
//!
//! The framework exposes three instrumentation seams, each a process-wide slot:
//!
//! * SQL — `rustasea_orm::profile::register_query_recorder`
//! * events — `rustasea_events::register_dispatch_observer`
//! * cache — [`recorders::wrap_cache_manager`] over a [`CacheManager`]
//!
//! [`install`] registers the SQL and event hooks (the cache wrap needs a live
//! manager and is applied separately). [`uninstall`] clears every hook, the
//! current-context slot, and the ring — the reset hook for tests.
//!
//! # Production safety
//!
//! Nothing here is installed unless [`install`] runs, and the middleware is a
//! no-op when `debug` is `false`, so a production process neither records
//! profiles nor exposes the `/_debugbar` surface (the app route answers `404`).

pub mod context;
pub mod middleware;
pub mod recorders;
pub mod ring;

pub use context::{
    clear_current_if, current, set_current, CacheEntry, EventEntry, ProfileContext, RequestProfile,
    SqlEntry,
};
pub use middleware::profiler_middleware;
pub use recorders::{
    wrap as wrap_store, wrap_cache_manager, EventsObserver, OrmQueryRecorder, RecordingStore,
};
pub use ring::{clear_ring, record, set_capacity, snapshot, DEFAULT_CAPACITY};

/// Serializes crate tests that touch the process-wide slots (context, ring,
/// hooks). The SQL/event hooks and the ring are shared across the whole test
/// binary, so every test that installs a hook or reads the ring takes this lock.
/// It is an async-aware mutex because several tests hold it across `.await`.
#[cfg(test)]
pub(crate) mod test_support {
    /// Crate-wide test serialization lock.
    pub static PROFILER_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
}

use std::sync::Arc;

use rustasea_orm::profile::{clear_query_recorder, register_query_recorder};

use rustasea_events::{clear_dispatch_observer, register_dispatch_observer};

/// Install the process-wide profiler: the ring buffer, the SQL recorder, and the
/// event observer.
///
/// The SQL and event hooks are replaced on every call (idempotent by
/// replacement), and the ring capacity is fixed on the first call. The cache
/// hook is **not** installed here — it needs a live [`CacheManager`], so call
/// [`wrap_cache_manager`] once the container's manager is available.
pub fn install(capacity: usize) {
    set_capacity(capacity);
    register_query_recorder(Arc::new(OrmQueryRecorder));
    register_dispatch_observer(Arc::new(EventsObserver));
}

/// Remove every profiler hook and drop the recorded profiles (test reset hook).
pub fn uninstall() {
    clear_query_recorder();
    clear_dispatch_observer();
    clear_ring();
}

#[cfg(test)]
mod tests;
