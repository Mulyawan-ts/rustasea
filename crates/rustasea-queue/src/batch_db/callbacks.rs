//! Process-wide batch lifecycle callbacks (`then`/`catch`/`finally`).
//!
//! Split out of `batch_db.rs` to keep that module within the file-size
//! standard. Callbacks are **in-process** (see the parent module docs): they are
//! registered here keyed by batch id and fire only in the process that observes
//! the batch reach the relevant state.
//!
//! ## Fire-once semantics
//!
//! Both `catch` (first failure) and the completion pair (`then`/`finally`) are
//! *consumed* from the map when fired — `catch` via [`Option::take`] and the
//! completion pair via [`take_callbacks`] — so a callback can never be skipped
//! or double-fired even when two workers observe the same transition.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use super::BatchRecord;

/// A batch lifecycle callback receiving the current [`BatchRecord`].
pub type BatchCallback = Arc<dyn Fn(&BatchRecord) + Send + Sync>;

/// The `then`/`catch`/`finally` callbacks registered for one batch.
#[derive(Default)]
pub(crate) struct CallbackSet {
    /// Fires once when the batch completes with no failures.
    pub(crate) then: Option<BatchCallback>,
    /// Fires once on the first recorded failure.
    pub(crate) catch: Option<BatchCallback>,
    /// Fires once when the batch completes, regardless of failures.
    pub(crate) finally: Option<BatchCallback>,
}

/// Process-wide callback map keyed by batch id.
static CALLBACKS: OnceLock<RwLock<HashMap<String, CallbackSet>>> = OnceLock::new();

/// Access the callback map, initializing it on first use.
fn callbacks() -> &'static RwLock<HashMap<String, CallbackSet>> {
    CALLBACKS.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Register a `then` callback (all jobs succeeded) for `batch_id`.
pub fn on_then<F>(batch_id: impl Into<String>, f: F)
where
    F: Fn(&BatchRecord) + Send + Sync + 'static,
{
    let id = batch_id.into();
    if let Ok(mut guard) = callbacks().write() {
        guard.entry(id).or_default().then = Some(Arc::new(f));
    }
}

/// Register a `catch` callback (first failure) for `batch_id`.
pub fn on_catch<F>(batch_id: impl Into<String>, f: F)
where
    F: Fn(&BatchRecord) + Send + Sync + 'static,
{
    let id = batch_id.into();
    if let Ok(mut guard) = callbacks().write() {
        guard.entry(id).or_default().catch = Some(Arc::new(f));
    }
}

/// Register a `finally` callback (always, on completion) for `batch_id`.
pub fn on_finally<F>(batch_id: impl Into<String>, f: F)
where
    F: Fn(&BatchRecord) + Send + Sync + 'static,
{
    let id = batch_id.into();
    if let Ok(mut guard) = callbacks().write() {
        guard.entry(id).or_default().finally = Some(Arc::new(f));
    }
}

/// Drop every callback registered for `batch_id`.
pub fn forget_batch_callbacks(batch_id: &str) {
    if let Ok(mut guard) = callbacks().write() {
        guard.remove(batch_id);
    }
}

/// Remove and return the callback set for `batch_id` (fire-once semantics).
pub(crate) fn take_callbacks(batch_id: &str) -> CallbackSet {
    callbacks()
        .write()
        .ok()
        .and_then(|mut guard| guard.remove(batch_id))
        .unwrap_or_default()
}

/// Fire the `catch` callback for `batch_id`, consuming it exactly once.
///
/// The closure is [`Option::take`]n out of the map under the write lock before
/// it is invoked, so even if two callers race to fire the first failure only
/// one observes `Some` and the callback can never double-fire. Returns whether a
/// callback was present and fired.
pub(crate) fn fire_catch(batch_id: &str, record: &BatchRecord) -> bool {
    let cb = callbacks()
        .write()
        .ok()
        .and_then(|mut guard| guard.get_mut(batch_id).and_then(|set| set.catch.take()));
    match cb {
        Some(cb) => {
            cb(record);
            true
        }
        None => false,
    }
}

/// Fire the completion callbacks: `then` (no failures) then `finally`.
///
/// Consumes the batch's whole callback set so completion fires at most once;
/// the caller must have already claimed the completion transition (see
/// [`super::DatabaseBatchRepository::finish`]).
pub(crate) fn fire_completion(batch_id: &str, record: &BatchRecord) {
    let set = take_callbacks(batch_id);
    if record.failed_jobs == 0 {
        if let Some(cb) = set.then {
            cb(record);
        }
    }
    if let Some(cb) = set.finally {
        cb(record);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Build a minimal finished record for callback assertions.
    fn record(failed_jobs: i64) -> BatchRecord {
        BatchRecord {
            id: "batch-test".to_string(),
            name: "test".to_string(),
            total_jobs: 1,
            pending_jobs: 0,
            failed_jobs,
            failed_job_ids: Vec::new(),
            options: serde_json::Value::Null,
            created_at: chrono::Utc::now(),
            finished_at: Some(chrono::Utc::now()),
            cancelled_at: None,
        }
    }

    /// `fire_catch` consumes the closure so it fires at most once.
    #[test]
    fn catch_fires_exactly_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        {
            let calls = calls.clone();
            on_catch("batch-catch-once", move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
            });
        }
        let rec = record(1);
        assert!(fire_catch("batch-catch-once", &rec), "first fire wins");
        assert!(
            !fire_catch("batch-catch-once", &rec),
            "second fire finds no closure"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1, "catch fires exactly once");
    }

    /// `fire_completion` consumes the whole set so `then`/`finally` fire once.
    #[test]
    fn completion_fires_exactly_once() {
        let then_calls = Arc::new(AtomicUsize::new(0));
        let finally_calls = Arc::new(AtomicUsize::new(0));
        {
            let then_calls = then_calls.clone();
            on_then("batch-complete-once", move |_| {
                then_calls.fetch_add(1, Ordering::SeqCst);
            });
        }
        {
            let finally_calls = finally_calls.clone();
            on_finally("batch-complete-once", move |_| {
                finally_calls.fetch_add(1, Ordering::SeqCst);
            });
        }
        let rec = record(0);
        fire_completion("batch-complete-once", &rec);
        fire_completion("batch-complete-once", &rec);
        assert_eq!(then_calls.load(Ordering::SeqCst), 1, "then fires once");
        assert_eq!(
            finally_calls.load(Ordering::SeqCst),
            1,
            "finally fires once"
        );
    }
}
