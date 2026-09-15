//! Query/model result caching with write invalidation (ADOPT-019).
//!
//! A [`QueryBuilder`](crate::builder::QueryBuilder) opts into caching with
//! `.cache(ttl)` / `.cache_forever()`; the executor stores the decoded rows
//! through a process-wide [`QueryCacheStore`] the application installs at boot
//! (an adapter over `rustasea-cache`'s `Repository`). Cache keys are versioned
//! per table: [`bump_table_generation`] advances a monotonic counter that the
//! key embeds, so any write to a table invalidates every cached query on it in
//! O(1) without scanning keys.
//!
//! The store seam mirrors [`crate::activity`]'s recorder slot — a
//! `OnceLock<RwLock<Option<Arc<dyn ...>>>>` installed with
//! [`register_cache_store`] and cleared with [`clear_cache_store`]. Caching is
//! strictly best-effort: a missing store (or a store error) degrades to a cache
//! miss, never a failed query.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, PoisonError, RwLock};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::builder::QueryBuilder;
use crate::error::Result;
use crate::types::Value;

/// Async key/value sink backing the query cache.
///
/// Implemented by an application adapter over `rustasea-cache` (or a test
/// double) and registered process-wide with [`register_cache_store`]. A `ttl`
/// of `None` means the entry never expires on its own; the table generation
/// still invalidates it on the next write.
#[async_trait::async_trait]
pub trait QueryCacheStore: Send + Sync {
    /// Fetch a cached payload by key, or `None` on a miss.
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;

    /// Store a payload under `key` with an optional expiry (`None` = forever).
    async fn put(&self, key: &str, value: Vec<u8>, ttl: Option<Duration>) -> Result<()>;

    /// Drop a single key.
    async fn forget(&self, key: &str) -> Result<()>;

    /// Drop every key scoped to `table` (bulk invalidation hint).
    async fn flush_table(&self, table: &str) -> Result<()>;
}

/// Process-wide cache-store slot, initialised on first use.
static CACHE_STORE: OnceLock<RwLock<Option<Arc<dyn QueryCacheStore>>>> = OnceLock::new();

/// The cache-store slot, initialised to empty on first use.
fn store_slot() -> &'static RwLock<Option<Arc<dyn QueryCacheStore>>> {
    CACHE_STORE.get_or_init(|| RwLock::new(None))
}

/// Install `store` as the process-wide query-cache backend.
///
/// Call once from application boot; a later call replaces the previous store. A
/// poisoned lock is recovered rather than surfaced, so a panic in another
/// thread cannot permanently disable caching.
pub fn register_cache_store(store: Arc<dyn QueryCacheStore>) {
    let mut guard = store_slot().write().unwrap_or_else(PoisonError::into_inner);
    *guard = Some(store);
}

/// Remove the process-wide cache store (bootstrap/test reset hook).
pub fn clear_cache_store() {
    let mut guard = store_slot().write().unwrap_or_else(PoisonError::into_inner);
    *guard = None;
}

/// The installed cache store, or `None` when caching is disabled.
///
/// The read path checks this first and skips key computation entirely when it is
/// `None`, so a query with no store pays no hashing cost.
pub fn cache_store() -> Option<Arc<dyn QueryCacheStore>> {
    let guard = store_slot().read().unwrap_or_else(PoisonError::into_inner);
    guard.clone()
}

/// Per-table generation counters driving versioned cache keys.
static TABLE_GENERATIONS: OnceLock<RwLock<HashMap<String, u64>>> = OnceLock::new();

/// The generation table, initialised to empty on first use.
fn generations_slot() -> &'static RwLock<HashMap<String, u64>> {
    TABLE_GENERATIONS.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Advance the generation for `table`, invalidating every key built against the
/// previous value.
pub fn bump_table_generation(table: &str) {
    let mut guard = generations_slot()
        .write()
        .unwrap_or_else(PoisonError::into_inner);
    let entry = guard.entry(table.to_string()).or_insert(0);
    *entry = entry.wrapping_add(1);
}

/// The current generation for `table` (`0` when never written).
pub fn table_generation(table: &str) -> u64 {
    let guard = generations_slot()
        .read()
        .unwrap_or_else(PoisonError::into_inner);
    guard.get(table).copied().unwrap_or(0)
}

/// Build the cache key for a query.
///
/// Format: `orm:{table}:{generation}:{hash32}`, where `hash32` is the first 16
/// bytes (32 hex chars) of the SHA-256 over `dialect|sql|bindings`. The
/// generation makes a table-wide invalidation a single counter bump, and the
/// hash keeps the key bounded regardless of SQL/binding length.
pub fn query_cache_key(
    table: &str,
    generation: u64,
    sql: &str,
    bindings: &[Value],
    dialect: &str,
) -> String {
    let payload = bindings_payload(bindings);
    let mut hasher = Sha256::new();
    hasher.update(dialect.as_bytes());
    hasher.update(b"|");
    hasher.update(sql.as_bytes());
    hasher.update(b"|");
    hasher.update(&payload);
    let digest = hasher.finalize();
    let hash = hex::encode(&digest[..16]);
    format!("orm:{table}:{generation}:{hash}")
}

/// Serialize bind values to a stable byte payload for hashing.
///
/// Each value renders through [`Value::to_json`], so the representation is
/// driver-independent. The `Vec<serde_json::Value>` serialization cannot fail;
/// the debug fallback exists only to keep the function total.
fn bindings_payload(bindings: &[Value]) -> Vec<u8> {
    let json: Vec<serde_json::Value> = bindings.iter().map(Value::to_json).collect();
    serde_json::to_vec(&json).unwrap_or_else(|_| format!("{bindings:?}").into_bytes())
}

/// Invalidate every cached query on `table` and flush the store's table scope.
///
/// The generation bump is unconditional (it is pure in-process state); the store
/// flush is skipped when no store is installed, so the hook is a no-op in a
/// cache-free build.
pub async fn flush_model_cache(table: &str) -> Result<()> {
    bump_table_generation(table);
    if let Some(store) = cache_store() {
        store.flush_table(table).await?;
    }
    Ok(())
}

/// Attach a model's default cache policy to a freshly built [`QueryBuilder`].
///
/// A `#[cacheable]` model caches by default: a `Some(ttl)` from
/// [`Model::cache_ttl`](crate::model::Model::cache_ttl) selects `.cache(ttl)`, a
/// `None` selects `.cache_forever()`. A non-cacheable model is returned
/// unchanged. The caller may still override with `.without_cache()` (or a
/// different `.cache(...)`) because the policy is attached at build time, not
/// execution time. Split out of [`crate::model`] to keep that file within the
/// file-size standard.
pub(crate) fn model_query(
    mut builder: QueryBuilder,
    cacheable: bool,
    ttl: Option<Duration>,
) -> QueryBuilder {
    if !cacheable {
        return builder;
    }
    builder = match ttl {
        Some(ttl) => builder.cache(ttl),
        None => builder.cache_forever(),
    };
    builder
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// In-memory store recording every operation for assertions.
    #[derive(Default)]
    struct RecordingStore {
        entries: Mutex<HashMap<String, Vec<u8>>>,
    }

    #[async_trait::async_trait]
    impl QueryCacheStore for RecordingStore {
        async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.entries.lock().unwrap().get(key).cloned())
        }

        async fn put(&self, key: &str, value: Vec<u8>, _ttl: Option<Duration>) -> Result<()> {
            self.entries.lock().unwrap().insert(key.to_string(), value);
            Ok(())
        }

        async fn forget(&self, key: &str) -> Result<()> {
            self.entries.lock().unwrap().remove(key);
            Ok(())
        }

        async fn flush_table(&self, _table: &str) -> Result<()> {
            Ok(())
        }
    }

    /// The key is deterministic for identical inputs.
    #[test]
    fn key_is_deterministic() {
        let bindings = vec![Value::Text("ada".into())];
        let a = query_cache_key("users", 0, "SELECT * FROM users", &bindings, "sqlite");
        let b = query_cache_key("users", 0, "SELECT * FROM users", &bindings, "sqlite");
        assert_eq!(a, b);
        assert!(a.starts_with("orm:users:0:"));
    }

    /// A generation bump changes the key, invalidating prior entries.
    #[test]
    fn generation_bump_changes_key() {
        let bindings: Vec<Value> = Vec::new();
        let before = query_cache_key("users", 0, "SELECT 1", &bindings, "sqlite");
        let after = query_cache_key("users", 1, "SELECT 1", &bindings, "sqlite");
        assert_ne!(before, after);
    }

    /// Different bindings produce different keys.
    #[test]
    fn bindings_change_key() {
        let a = query_cache_key("users", 0, "SELECT 1", &[Value::Int(1)], "sqlite");
        let b = query_cache_key("users", 0, "SELECT 1", &[Value::Int(2)], "sqlite");
        assert_ne!(a, b);
    }

    /// Registering then clearing the slot round-trips through the accessor.
    #[test]
    fn store_slot_round_trip() {
        clear_cache_store();
        assert!(cache_store().is_none());
        register_cache_store(Arc::new(RecordingStore::default()));
        assert!(cache_store().is_some());
        clear_cache_store();
        assert!(cache_store().is_none());
    }

    /// The generation accessor tracks bumps and is independent per table.
    #[test]
    fn generation_tracks_bumps() {
        let table = "generation_tracks_bumps_table";
        let start = table_generation(table);
        bump_table_generation(table);
        assert_eq!(table_generation(table), start + 1);
    }
}
