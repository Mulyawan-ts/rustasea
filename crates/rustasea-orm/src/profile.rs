//! SQL query instrumentation hook (ADOPT-009).
//!
//! The ORM exposes a single, cheap seam for a request profiler (debug toolbar):
//! [`track`] wraps a driver call and, **only when a recorder is installed**,
//! measures its wall-clock duration and reports a [`SqlQueryEvent`]. With no
//! recorder registered the wrapper performs a single [`OnceLock`] read and runs
//! the future untouched, so production pays no serialization or timing cost.
//!
//! This mirrors the process-wide slot shape of [`crate::activity`] (a
//! poison-recovering [`RwLock`] behind a [`OnceLock`]) and the recorder is a
//! **synchronous** trait: timing is captured with [`Instant::now`] before the
//! `await` and the event is recorded after it, so no async trait object is
//! needed on the hot path.

use std::future::Future;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

use crate::error::Result;

/// One measured SQL driver call.
#[derive(Debug, Clone)]
pub struct SqlQueryEvent {
    /// Call kind: `"fetch_json"` (read) or `"execute_bind"` (write).
    pub kind: &'static str,
    /// The original SQL text (before any placeholder adaptation).
    pub sql: String,
    /// Wall-clock duration of the driver call.
    pub duration: Duration,
    /// Driver error rendered as a string, or `None` on success.
    pub error: Option<String>,
}

/// Synchronous sink for measured SQL calls.
///
/// Implemented by the debug toolbar's recorder and registered process-wide via
/// [`register_query_recorder`]. Implementations must be `Send + Sync` so the
/// recorder can be shared across the async execution path, and `record` must be
/// cheap (the profiler pushes into an in-memory buffer).
pub trait QueryRecorder: Send + Sync {
    /// Record one measured SQL call.
    fn record(&self, event: SqlQueryEvent);
}

/// Process-wide recorder slot, initialised on first use.
static RECORDER: OnceLock<RwLock<Option<Arc<dyn QueryRecorder>>>> = OnceLock::new();

/// The recorder slot, initialised to empty on first use.
fn recorder_slot() -> &'static RwLock<Option<Arc<dyn QueryRecorder>>> {
    RECORDER.get_or_init(|| RwLock::new(None))
}

/// Install `recorder` as the process-wide SQL recorder.
///
/// Call once from application boot; a later call replaces the previous
/// recorder. A poisoned lock is recovered rather than surfaced, so a panic in
/// another thread cannot permanently disable profiling.
pub fn register_query_recorder(recorder: Arc<dyn QueryRecorder>) {
    let slot = recorder_slot();
    let mut guard = slot
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *guard = Some(recorder);
}

/// Remove the process-wide SQL recorder (bootstrap/test reset hook).
pub fn clear_query_recorder() {
    let slot = recorder_slot();
    let mut guard = slot
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *guard = None;
}

/// The installed SQL recorder, or `None` when profiling is disabled.
///
/// The execution path checks this first and runs the driver call untouched when
/// it is `None`, so an uninstrumented build pays no timing or allocation cost.
pub fn query_recorder() -> Option<Arc<dyn QueryRecorder>> {
    let slot = recorder_slot();
    let guard = slot
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.clone()
}

/// Run `run`, reporting its duration and outcome to the installed recorder.
///
/// When no recorder is installed `run` is awaited untouched (a single slot
/// read). Otherwise the call is timed with [`Instant::now`], the future is
/// awaited, and a [`SqlQueryEvent`] carrying `kind`, the **original** `sql`
/// text, the elapsed duration, and any error string is recorded. The original
/// error is returned unchanged, so instrumentation never alters behaviour.
pub async fn track<R>(
    kind: &'static str,
    sql: &str,
    run: impl Future<Output = Result<R>>,
) -> Result<R> {
    let Some(recorder) = query_recorder() else {
        return run.await;
    };
    let started = Instant::now();
    let outcome = run.await;
    let duration = started.elapsed();
    let error = outcome.as_ref().err().map(ToString::to_string);
    recorder.record(SqlQueryEvent {
        kind,
        sql: sql.to_string(),
        duration,
        error,
    });
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies the registry round-trips an installed recorder.
    #[test]
    fn recorder_registry_round_trips() {
        struct Noop;
        impl QueryRecorder for Noop {
            fn record(&self, _event: SqlQueryEvent) {}
        }
        clear_query_recorder();
        assert!(query_recorder().is_none());
        register_query_recorder(Arc::new(Noop));
        assert!(query_recorder().is_some());
        clear_query_recorder();
        assert!(query_recorder().is_none());
    }

    /// Verifies `track` forwards the SQL text, kind, and success state.
    #[tokio::test]
    async fn track_records_success_with_original_sql() {
        use std::sync::Mutex;

        #[derive(Default)]
        struct Capturing {
            events: Mutex<Vec<SqlQueryEvent>>,
        }
        impl QueryRecorder for Capturing {
            fn record(&self, event: SqlQueryEvent) {
                self.events.lock().unwrap().push(event);
            }
        }

        let recorder = Arc::new(Capturing::default());
        register_query_recorder(recorder.clone());
        let value = track("fetch_json", "SELECT $1", async {
            Ok::<_, crate::error::OrmError>(7)
        })
        .await
        .unwrap();
        clear_query_recorder();

        assert_eq!(value, 7);
        let events = recorder.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "fetch_json");
        assert_eq!(events[0].sql, "SELECT $1");
        assert!(events[0].error.is_none());
    }

    /// Verifies `track` runs untouched (and records nothing) with no recorder.
    #[tokio::test]
    async fn track_without_recorder_passes_through() {
        clear_query_recorder();
        let value = track("execute_bind", "DELETE FROM t", async {
            Ok::<_, crate::error::OrmError>(3)
        })
        .await
        .unwrap();
        assert_eq!(value, 3);
    }
}
