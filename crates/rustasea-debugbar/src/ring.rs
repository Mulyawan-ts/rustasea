//! Bounded in-memory ring of finalized request profiles.
//!
//! The ring keeps the newest [`crate::DEFAULT_CAPACITY`] profiles so the toolbar
//! can list recent requests without the dev process growing without limit. The
//! eviction shape mirrors `rustasea_queue::notification`'s skipped sink: a
//! [`VecDeque`] behind a [`Mutex`], oldest evicted on push.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use crate::context::RequestProfile;

/// Default number of request profiles retained by the ring.
pub const DEFAULT_CAPACITY: usize = 128;

/// The process-wide ring buffer.
static RING: OnceLock<Mutex<VecDeque<RequestProfile>>> = OnceLock::new();

/// Configured ring capacity; falls back to [`DEFAULT_CAPACITY`] when unset.
static CAPACITY: OnceLock<usize> = OnceLock::new();

/// Access the ring, initialising it to the default capacity on first use.
fn ring() -> &'static Mutex<VecDeque<RequestProfile>> {
    RING.get_or_init(|| Mutex::new(VecDeque::with_capacity(DEFAULT_CAPACITY)))
}

/// The configured ring capacity (at least one).
fn capacity() -> usize {
    CAPACITY.get().copied().unwrap_or(DEFAULT_CAPACITY).max(1)
}

/// Set the ring capacity (first call wins; later calls are ignored).
///
/// Called by [`crate::install`]; the ring never shrinks below one entry.
pub fn set_capacity(capacity: usize) {
    let _ = CAPACITY.set(capacity.max(1));
}

/// Record a finalized profile, evicting the oldest when at capacity.
///
/// The lock is poison-tolerant: a panic while holding it cannot permanently
/// disable the toolbar.
pub fn record(profile: RequestProfile) {
    let mut guard = ring()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let capacity = capacity();
    while guard.len() >= capacity {
        guard.pop_front();
    }
    guard.push_back(profile);
}

/// Snapshot the recorded profiles (newest last, at most [`DEFAULT_CAPACITY`]).
pub fn snapshot() -> Vec<RequestProfile> {
    let guard = ring()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.iter().cloned().collect()
}

/// Drop every recorded profile (bootstrap/test reset hook).
pub fn clear_ring() {
    let mut guard = ring()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal profile with the given path.
    fn profile(path: &str) -> RequestProfile {
        RequestProfile {
            method: "GET".to_string(),
            path: path.to_string(),
            request_id: None,
            status: 200,
            duration_ms: 1.0,
            queries: Vec::new(),
            cache_ops: Vec::new(),
            events: Vec::new(),
            started_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    /// Verifies the ring evicts the oldest profile beyond capacity.
    #[tokio::test]
    async fn ring_evicts_oldest_beyond_capacity() {
        let _guard = crate::test_support::PROFILER_LOCK.lock().await;
        clear_ring();
        let cap = capacity();
        record(profile("/first"));
        let mut over = VecDeque::with_capacity(cap);
        for _ in 0..cap {
            over.push_back(profile("/seed"));
        }
        {
            let mut guard = ring().lock().unwrap_or_else(|p| p.into_inner());
            *guard = over;
        }
        record(profile("/last"));

        let snapshot = snapshot();
        assert_eq!(snapshot.len(), cap);
        assert!(
            !snapshot.iter().any(|p| p.path == "/first"),
            "oldest pre-seed profile evicted"
        );
        assert!(
            snapshot.iter().any(|p| p.path == "/last"),
            "newest profile retained"
        );
        clear_ring();
    }
}
