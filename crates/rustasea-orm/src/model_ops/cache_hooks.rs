//! Write-path cache invalidation (ADOPT-019).
//!
//! Every successful mutation bumps the affected table's generation, so any
//! cached query on that table misses on the next read. The hook is gated on a
//! store being installed **only** — not on the model opting into `#[cacheable]`
//! — because an explicit `.cache()` on any table must be invalidated when that
//! table is written, regardless of the model's declared default.

/// Invalidate every cached query on `table`.
///
/// A no-op when no cache store is installed, so a cache-free build pays a single
/// `OnceLock` read per write. Split out of [`super`] so the write path stays
/// within the file-size standard.
pub(crate) fn invalidate_table(table: &str) {
    if crate::cache::cache_store().is_some() {
        crate::cache::bump_table_generation(table);
    }
}
