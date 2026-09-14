//! Process-wide application route-table registry for `route:list`.
//!
//! `route:list` renders the *live* application route table, but `rustasea-cli`
//! is framework-generic: it must not depend on an application crate, or the
//! dependency would invert and create a workspace cycle. Applications therefore
//! publish their table here during boot and the command reads it back.
//!
//! The slot stores a closure rather than a snapshot, so every `route:list` run
//! reflects the table as the running application built it. The slot is a
//! [`OnceLock`]`<`[`RwLock`]`<Option<RouteSource>>>`: a write-once global whose
//! value may be replaced (the application boot is the only production writer)
//! and cleared by tests for isolation.

use std::sync::{Arc, OnceLock, RwLock};

use rustasea_router::RouteEntry;

/// A callable that yields the current application route table.
pub type RouteSource = Arc<dyn Fn() -> Vec<RouteEntry> + Send + Sync>;

/// The process-wide route-source slot.
static ROUTE_SOURCE: OnceLock<RwLock<Option<RouteSource>>> = OnceLock::new();

/// Borrow the process-wide route-source slot, initializing it on first use.
fn slot() -> &'static RwLock<Option<RouteSource>> {
    ROUTE_SOURCE.get_or_init(|| RwLock::new(None))
}

/// Publish the application's live route source for `route:list`.
///
/// `source` is invoked on every `route:list` run, so the command reports the
/// table as it exists at call time. A later call replaces an earlier source.
pub fn set_route_source(source: impl Fn() -> Vec<RouteEntry> + Send + Sync + 'static) {
    if let Ok(mut guard) = slot().write() {
        *guard = Some(Arc::new(source));
    }
}

/// Publish a static route-table snapshot for `route:list`.
///
/// Convenience for callers that already hold the table; the snapshot is cloned
/// on every read.
pub fn set_routes(routes: Vec<RouteEntry>) {
    set_route_source(move || routes.clone());
}

/// Return the current application route table.
///
/// Empty when no source has been published — for example the framework-generic
/// `cargo-artisan` binary, which never boots an application — or when the lock
/// is poisoned. Never panics.
pub fn routes() -> Vec<RouteEntry> {
    match slot().read() {
        Ok(guard) => guard.as_ref().map(|source| source()).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// Remove the published route source (test isolation).
pub fn clear_route_source() {
    if let Ok(mut guard) = slot().write() {
        *guard = None;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use super::*;

    /// Serializes tests that share the process-wide registry.
    static LOCK: Mutex<()> = Mutex::new(());

    /// Build a minimal route entry at `path`.
    fn entry(path: &str) -> RouteEntry {
        RouteEntry {
            method: "GET".into(),
            path: path.into(),
            name: None,
            middleware: Vec::new(),
            domain: None,
            binding_fields: Vec::new(),
            controller: None,
            handler: None,
        }
    }

    /// A published snapshot is returned verbatim until cleared.
    #[test]
    fn published_snapshot_is_readable() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_routes(vec![entry("/home")]);
        assert_eq!(routes().len(), 1);
        assert_eq!(routes()[0].path, "/home");

        clear_route_source();
        assert!(routes().is_empty());
    }

    /// A closure source is re-evaluated on every read.
    #[test]
    fn closure_source_is_live() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        set_route_source(move || {
            let call = counter.fetch_add(1, Ordering::SeqCst);
            vec![entry(&format!("/call-{call}"))]
        });

        assert_eq!(routes()[0].path, "/call-0");
        assert_eq!(routes()[0].path, "/call-1");
        clear_route_source();
    }
}
