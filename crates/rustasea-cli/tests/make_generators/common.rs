//! Shared fixtures for the `make:*` generator smoke tests.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Unique temp root per invocation, removed on success.
pub fn temp_root() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "rustasea-cli-make-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    root
}
