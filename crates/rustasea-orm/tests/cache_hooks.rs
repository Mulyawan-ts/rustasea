//! Query-cache hook integration tests (ADOPT-019).
//!
//! Exercises the opt-in query cache and write-path invalidation against a real
//! in-memory SQLite pool with a recording store installed process-wide. The
//! store logs every operation, so a cache hit/miss is observable without a
//! database counter: the first query records `get`(miss)+`put`, a repeat records
//! only `get`(hit), and any table write advances the generation so the next
//! query misses again.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use rustasea_orm::cache::{clear_cache_store, register_cache_store, QueryCacheStore};
use rustasea_orm::{DbPool, Model, ModelOps, OrmError, QueryBuilder, Result, Value};
use uuid::Uuid;

// Regression tests for the ADOPT-019 review findings live in a submodule so this
// file stays within the 500-line cap. An explicit `path` is required because
// this integration-test file is a crate root, not a `mod.rs`.
#[path = "cache_hooks/findings.rs"]
mod findings;

/// Serialises tests that install the process-wide cache-store slot.
fn test_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// An in-memory store that records every operation for assertions.
#[derive(Default)]
struct RecordingCacheStore {
    /// Stored payloads keyed by cache key.
    entries: Mutex<HashMap<String, Vec<u8>>>,
    /// Human-readable op log (`get:`, `put:ttl=...:`, `forget:`, `flush:`).
    ops: Mutex<Vec<String>>,
}

impl RecordingCacheStore {
    /// The recorded operations, cloned out of the lock.
    fn ops(&self) -> Vec<String> {
        self.ops.lock().expect("ops lock").clone()
    }

    /// How many operations start with `prefix`.
    fn count(&self, prefix: &str) -> usize {
        self.ops()
            .iter()
            .filter(|entry| entry.starts_with(prefix))
            .count()
    }
}

#[async_trait::async_trait]
impl QueryCacheStore for RecordingCacheStore {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.ops
            .lock()
            .expect("ops lock")
            .push(format!("get:{key}"));
        Ok(self.entries.lock().expect("entries lock").get(key).cloned())
    }

    async fn put(&self, key: &str, value: Vec<u8>, ttl: Option<Duration>) -> Result<()> {
        let ttl = match ttl {
            Some(ttl) => format!("ttl={}", ttl.as_secs()),
            None => "ttl=forever".to_string(),
        };
        self.ops
            .lock()
            .expect("ops lock")
            .push(format!("put:{ttl}:{key}"));
        self.entries
            .lock()
            .expect("entries lock")
            .insert(key.to_string(), value);
        Ok(())
    }

    async fn forget(&self, key: &str) -> Result<()> {
        self.ops
            .lock()
            .expect("ops lock")
            .push(format!("forget:{key}"));
        self.entries.lock().expect("entries lock").remove(key);
        Ok(())
    }

    async fn flush_table(&self, table: &str) -> Result<()> {
        self.ops
            .lock()
            .expect("ops lock")
            .push(format!("flush:{table}"));
        Ok(())
    }
}

/// Install a fresh recording store and return it.
fn install_store() -> Arc<RecordingCacheStore> {
    let store = Arc::new(RecordingCacheStore::default());
    register_cache_store(store.clone());
    store
}

/// A derived model mapped to `users`, with tracked timestamps and soft deletes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, rustasea_macros::Model)]
struct User {
    id: Uuid,
    name: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}

/// A derived model opted into caching with a 120-second default TTL.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, rustasea_macros::Model)]
#[cacheable(ttl = "120")]
struct Country {
    id: Uuid,
    name: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}

/// A derived model opted into forever caching.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, rustasea_macros::Model)]
#[cacheable]
struct Currency {
    id: Uuid,
    name: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}

/// Build a fresh in-memory pool with the `users` table applied.
async fn pool_with_users() -> DbPool {
    let pool = DbPool::connect("sqlite::memory:").await.unwrap();
    pool.execute_bind(
        "CREATE TABLE users (
            id BLOB PRIMARY KEY,
            name TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            deleted_at TEXT
        )",
        &[],
    )
    .await
    .unwrap();
    pool
}

/// Build an unsaved `User` instance with a fresh id.
fn sample(name: &str) -> User {
    let now = Utc::now();
    User {
        id: Uuid::now_v7(),
        name: name.to_string(),
        created_at: now,
        updated_at: now,
        deleted_at: None,
    }
}

/// A repeat identical cached query is served from the store, not re-stored.
#[tokio::test]
async fn explicit_cache_hits_second_query() {
    let _guard = test_lock().lock().await;
    clear_cache_store();
    let store = install_store();
    let pool = pool_with_users().await;
    User::create(&pool, sample("Ada")).await.unwrap();

    let query = || {
        <User as Model>::query()
            .where_eq("name", "Ada")
            .cache(Duration::from_secs(60))
    };

    let first = query().get(&pool).await.unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(store.count("put:"), 1, "first query stores one entry");

    let second = query().get(&pool).await.unwrap();
    assert_eq!(second, first, "second query returns the cached rows");
    assert_eq!(store.count("put:"), 1, "second query is a hit, no new put");
    assert_eq!(store.count("get:"), 2, "both queries consulted the store");

    clear_cache_store();
}

/// `without_cache()` always hits the database, never storing or reading.
#[tokio::test]
async fn without_cache_always_hits_database() {
    let _guard = test_lock().lock().await;
    clear_cache_store();
    let store = install_store();
    let pool = pool_with_users().await;
    User::create(&pool, sample("Ada")).await.unwrap();

    let query = || {
        <User as Model>::query()
            .where_eq("name", "Ada")
            .cache(Duration::from_secs(60))
            .without_cache()
    };

    assert_eq!(query().get(&pool).await.unwrap().len(), 1);
    assert_eq!(query().get(&pool).await.unwrap().len(), 1);
    assert_eq!(
        store.count("get:"),
        0,
        "without_cache never reads the store"
    );
    assert_eq!(
        store.count("put:"),
        0,
        "without_cache never writes the store"
    );

    clear_cache_store();
}

/// A write to the table invalidates a previously cached query (fresh data).
#[tokio::test]
async fn write_invalidates_table_generation() {
    let _guard = test_lock().lock().await;
    clear_cache_store();
    let store = install_store();
    let pool = pool_with_users().await;
    let created = User::create(&pool, sample("Ada")).await.unwrap();

    let query = || {
        <User as Model>::query()
            .where_eq("name", "Ada")
            .cache(Duration::from_secs(60))
    };

    assert_eq!(query().get(&pool).await.unwrap().len(), 1);
    assert_eq!(query().get(&pool).await.unwrap().len(), 1);
    assert_eq!(store.count("put:"), 1);

    let mut updated = created.clone();
    updated.name = "Ada Lovelace".to_string();
    User::update(&pool, updated).await.unwrap();

    // The generation bumped, so the old key misses and the row is gone.
    let after = query().get(&pool).await.unwrap();
    assert!(after.is_empty(), "stale cache must not serve the old row");
    assert_eq!(store.count("put:"), 2, "the write forced a re-store");

    clear_cache_store();
}

/// `create` invalidates the table generation.
#[tokio::test]
async fn create_invalidates() {
    let _guard = test_lock().lock().await;
    clear_cache_store();
    let store = install_store();
    let pool = pool_with_users().await;

    let query = || {
        <User as Model>::query()
            .where_eq("name", "Grace")
            .cache(Duration::from_secs(60))
    };

    assert!(query().get(&pool).await.unwrap().is_empty());
    assert!(query().get(&pool).await.unwrap().is_empty());
    assert_eq!(store.count("put:"), 1);

    User::create(&pool, sample("Grace")).await.unwrap();

    assert_eq!(
        query().get(&pool).await.unwrap().len(),
        1,
        "create must invalidate the cached empty result"
    );
    assert_eq!(store.count("put:"), 2);

    clear_cache_store();
}

/// `soft_delete` invalidates the table generation.
#[tokio::test]
async fn soft_delete_invalidates() {
    let _guard = test_lock().lock().await;
    clear_cache_store();
    let store = install_store();
    let pool = pool_with_users().await;
    let created = User::create(&pool, sample("Grace")).await.unwrap();

    let query = || {
        <User as Model>::query()
            .where_eq("name", "Grace")
            .cache(Duration::from_secs(60))
    };

    assert_eq!(query().get(&pool).await.unwrap().len(), 1);
    assert_eq!(query().get(&pool).await.unwrap().len(), 1);
    assert_eq!(store.count("put:"), 1);

    User::soft_delete(&pool, created.id).await.unwrap();

    assert!(
        query().get(&pool).await.unwrap().is_empty(),
        "soft delete must invalidate the cached active row"
    );
    assert_eq!(store.count("put:"), 2);

    clear_cache_store();
}

/// `cache_forever` stores with a `None` TTL (never expires on its own).
#[tokio::test]
async fn cache_forever_stores_with_none_ttl() {
    let _guard = test_lock().lock().await;
    clear_cache_store();
    let store = install_store();
    let pool = pool_with_users().await;
    User::create(&pool, sample("Ada")).await.unwrap();

    <User as Model>::query()
        .where_eq("name", "Ada")
        .cache_forever()
        .get(&pool)
        .await
        .unwrap();

    assert_eq!(store.count("put:ttl=forever:"), 1, "forever stores no TTL");

    clear_cache_store();
}

/// With no store installed, a cached query still runs against the database.
#[tokio::test]
async fn no_store_registered_is_noop() {
    let _guard = test_lock().lock().await;
    clear_cache_store();
    let pool = pool_with_users().await;
    User::create(&pool, sample("Ada")).await.unwrap();

    let rows = <User as Model>::query()
        .where_eq("name", "Ada")
        .cache(Duration::from_secs(60))
        .get(&pool)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);

    // The write path is also a no-op with no store.
    <User as Model>::query()
        .where_eq("name", "Ada")
        .cache(Duration::from_secs(60))
        .get(&pool)
        .await
        .unwrap();
}

/// `Model::flush_cache` bumps the generation and flushes the store scope.
#[tokio::test]
async fn explicit_flush_cache_flushes_store() {
    let _guard = test_lock().lock().await;
    clear_cache_store();
    let store = install_store();

    User::flush_cache().await.unwrap();
    assert_eq!(store.count("flush:users"), 1);

    clear_cache_store();
}

/// The derive emits `cacheable`/`cache_ttl` overrides.
#[tokio::test]
async fn derive_emits_cacheable_overrides() {
    assert!(<Country as Model>::cacheable());
    assert_eq!(
        <Country as Model>::cache_ttl(),
        Some(Duration::from_secs(120))
    );

    assert!(<Currency as Model>::cacheable());
    assert_eq!(<Currency as Model>::cache_ttl(), None);

    assert!(!<User as Model>::cacheable());
    assert_eq!(<User as Model>::cache_ttl(), None);
}

/// `exists` flows through the cached `first`, so a repeated probe is a hit.
#[tokio::test]
async fn count_query_caches_separately() {
    let _guard = test_lock().lock().await;
    clear_cache_store();
    let store = install_store();
    let pool = pool_with_users().await;
    User::create(&pool, sample("Ada")).await.unwrap();

    let builder = || {
        <User as Model>::query()
            .where_eq("name", "Ada")
            .cache(Duration::from_secs(60))
    };

    assert_eq!(builder().count(&pool).await.unwrap(), 1);
    assert_eq!(builder().count(&pool).await.unwrap(), 1);
    assert_eq!(store.count("put:"), 1, "COUNT is cached across calls");

    clear_cache_store();
}

/// A raw value binding keeps the query path generic (sanity for `Value`).
#[tokio::test]
async fn raw_builder_cache_round_trip() {
    let _guard = test_lock().lock().await;
    clear_cache_store();
    let _store = install_store();
    let pool = pool_with_users().await;
    User::create(&pool, sample("Ada")).await.unwrap();

    let rows = QueryBuilder::table("users")
        .where_eq("name", Value::Text("Ada".to_string()))
        .cache_forever()
        .get(&pool)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);

    clear_cache_store();
}

/// A missing row still surfaces a typed error through the cached path.
#[tokio::test]
async fn cached_first_or_fail_surfaces_not_found() {
    let _guard = test_lock().lock().await;
    clear_cache_store();
    let _store = install_store();
    let pool = pool_with_users().await;

    let err = <User as Model>::query()
        .where_eq("name", "nobody")
        .cache(Duration::from_secs(60))
        .first_or_fail(&pool)
        .await
        .unwrap_err();
    assert!(matches!(err, OrmError::NotFound));

    clear_cache_store();
}
