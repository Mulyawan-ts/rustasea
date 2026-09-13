/// Cache repository facade — typed K/V, remember/forever, store selection.
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::config::{CacheConfig, CacheDriver};
use crate::error::{CacheConfigError, CacheError, Result};
use crate::lock::Lock;
use crate::memory::MemoryStore;
use crate::redis::RedisStore;
use crate::store::{Store, DEFAULT_PREFIX};

/// Canonical store names.
pub const MEMORY_STORE: &str = "memory";
/// Canonical store names.
pub const REDIS_STORE: &str = "redis";

/// Shared reference to a store.
pub type StoreRef = Arc<dyn Store>;

/// Registry of named stores plus the typed repository facade.
///
/// Stores are isolated by name: a `put` on `redis` is not visible on
/// `memory` (US-M4-03 decision table). The manager seeds both canonical stores
/// so `store(name)` never surprises; extra stores can be registered.
///
/// The manager also carries the configured default store name and key prefix,
/// applied to every [`Repository`] it hands out. [`CacheManager::new`] keeps
/// the historical behaviour (default `memory`, [`DEFAULT_PREFIX`]);
/// [`CacheManager::from_config`] derives both from a [`CacheConfig`].
#[derive(Clone)]
pub struct CacheManager {
    stores: Arc<std::sync::RwLock<HashMap<String, StoreRef>>>,
    default_store: String,
    prefix: String,
}

impl CacheManager {
    /// Create a manager pre-seeded with isolated `memory` + `redis` stores.
    ///
    /// Uses the built-in default store (`memory`) and prefix (`-cache-`), i.e.
    /// identical to the historical constructor.
    pub fn new() -> Self {
        let mut map = HashMap::new();
        map.insert(
            MEMORY_STORE.to_string(),
            Arc::new(MemoryStore::new()) as StoreRef,
        );
        map.insert(
            REDIS_STORE.to_string(),
            Arc::new(RedisStore::new(REDIS_STORE)) as StoreRef,
        );
        Self {
            stores: Arc::new(std::sync::RwLock::new(map)),
            default_store: MEMORY_STORE.to_string(),
            prefix: DEFAULT_PREFIX.to_string(),
        }
    }

    /// Build a manager from a typed [`CacheConfig`].
    ///
    /// The `memory` store is always registered. A declared `redis` store is
    /// registered only when the crate is built with the `redis` feature AND the
    /// store resolves a URL (`url`, a URL-shaped `connection`, or `REDIS_URL`);
    /// otherwise it is skipped so a config that merely mentions Redis does not
    /// poison an otherwise-working process. Selecting an unimplemented driver
    /// (`database`, `file`, …) or naming an undeclared default store returns a
    /// typed error, so misconfiguration fails closed.
    ///
    /// # Errors
    ///
    /// [`CacheConfigError`] promoted to [`CacheError::Config`] for an unknown
    /// default store or an unsupported/unknown driver.
    pub fn from_config(config: &CacheConfig) -> Result<Self> {
        config.validate()?;
        let mut map: HashMap<String, StoreRef> = HashMap::new();
        map.insert(
            MEMORY_STORE.to_string(),
            Arc::new(MemoryStore::new()) as StoreRef,
        );

        for (name, store) in &config.stores {
            match store.driver_kind()? {
                CacheDriver::Memory => {
                    map.entry(name.clone())
                        .or_insert_with(|| Arc::new(MemoryStore::new()) as StoreRef);
                }
                CacheDriver::Redis => {
                    #[cfg(feature = "redis")]
                    if let Some(url) = store.resolved_url() {
                        let redis = RedisStore::from_url(name, &url)?;
                        map.insert(name.clone(), Arc::new(redis) as StoreRef);
                    }
                    // Feature disabled, or no URL configured: the store is
                    // intentionally left unregistered (spec: register `redis`
                    // only when the feature is enabled AND a url is present).
                    // `store(name)` on it still yields a typed UnknownStore.
                    #[cfg(not(feature = "redis"))]
                    let _ = store;
                }
            }
        }

        Ok(Self {
            stores: Arc::new(std::sync::RwLock::new(map)),
            default_store: config.default.clone(),
            prefix: config.prefix.clone(),
        })
    }

    /// Convenience: load `[cache]` from a loader, then build the manager.
    ///
    /// # Errors
    ///
    /// Any [`CacheConfigError`] from parsing/validating the config, promoted to
    /// [`CacheError::Config`].
    pub fn configure_from(loader: &rustasea_config::ConfigLoader) -> Result<Self> {
        let config = CacheConfig::from_loader(loader)?;
        Self::from_config(&config)
    }

    /// Register (or replace) a store under `name`.
    pub fn register(&self, name: impl Into<String>, store: StoreRef) {
        if let Ok(mut map) = self.stores.write() {
            map.insert(name.into(), store);
        }
    }

    /// Name of the default store used by [`CacheManager::repository`].
    pub fn default_store(&self) -> &str {
        &self.default_store
    }

    /// Key prefix applied to every repository handed out by this manager.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Resolve the store registered under `name`.
    pub fn store(&self, name: &str) -> Result<Repository> {
        let map = self
            .stores
            .read()
            .map_err(|_| CacheError::UnknownStore("cache manager registry poisoned".into()))?;
        let store = map
            .get(name)
            .cloned()
            .ok_or_else(|| CacheError::UnknownStore(name.to_string()))?;
        Ok(Repository::new(store).with_prefix(&self.prefix))
    }

    /// Access the default store repository.
    ///
    /// Uses the configured default store (Laravel `cache.default`); falls back
    /// to `memory` when that store is not registered.
    pub fn repository(&self) -> Result<Repository> {
        match self.store(&self.default_store) {
            Ok(repository) => Ok(repository),
            Err(_) if self.default_store != MEMORY_STORE => self.store(MEMORY_STORE),
            Err(error) => Err(error),
        }
    }

    /// Access a context-scoped default-store repository.
    ///
    /// Mirrors Laravel's `Cache::withContext([...])`: every key passing through
    /// the returned repository carries a deterministic context segment, so
    /// tenant-scoped writes are invisible to unscoped reads and vice versa.
    pub fn with_context(&self, context: HashMap<String, String>) -> Result<Repository> {
        Ok(self.repository()?.with_context(context))
    }

    /// Access a named store repository (same as `store`).
    pub fn cache(&self, name: &str) -> Result<Repository> {
        self.store(name)
    }

    /// Create a `Lock` over the store registered under `name`.
    pub fn lock(&self, name: &str, key: impl Into<String>, ttl: Duration) -> Result<Lock> {
        let map = self
            .stores
            .read()
            .map_err(|_| CacheError::UnknownStore("cache manager registry poisoned".into()))?;
        let store = map
            .get(name)
            .cloned()
            .ok_or_else(|| CacheError::UnknownStore(name.to_string()))?;
        Ok(Lock::new(store, key, ttl))
    }
}

impl Default for CacheManager {
    /// Create a manager with isolated default stores.
    fn default() -> Self {
        Self::new()
    }
}

/// Typed, prefix-aware view over one store.
///
/// Applies a key prefix (the `-cache-` marker by default, or the configured
/// `cache.prefix`) to every key and (de)serializes values as JSON
/// (`Repository` = typed layer; `Store` = bytes).
#[derive(Clone)]
pub struct Repository {
    store: StoreRef,
    context: Option<String>,
    prefix: String,
}

impl Repository {
    /// Create a repository over an existing store.
    ///
    /// Uses the built-in [`DEFAULT_PREFIX`]; call [`Repository::with_prefix`]
    /// to override it.
    pub fn new(store: StoreRef) -> Self {
        Self {
            store,
            context: None,
            prefix: DEFAULT_PREFIX.to_string(),
        }
    }

    /// Return a repository that applies `prefix` to every key.
    ///
    /// The prefix is validated to contain the hyphenated `-cache-` marker (the
    /// same rule `SessionPolicy` enforces) so cache keys can never be confused
    /// with session keys.
    ///
    /// # Errors
    ///
    /// [`CacheConfigError::InvalidPrefix`] promoted to [`CacheError::Config`]
    /// when `prefix` lacks the `-cache-` marker.
    pub fn try_with_prefix(mut self, prefix: impl Into<String>) -> Result<Self> {
        let prefix = prefix.into();
        if !prefix.contains("-cache-") {
            return Err(CacheConfigError::InvalidPrefix(prefix).into());
        }
        self.prefix = prefix;
        Ok(self)
    }

    /// Infallible builder used internally by [`CacheManager`] with an
    /// already-validated prefix.
    fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Key prefix applied by this repository.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Return a repository whose keys carry a deterministic context segment.
    ///
    /// Laravel's `Cache::withContext` prefixes keys with the supplied context
    /// map. Here the map is normalized into one stable segment — values are
    /// percent-encoded (`=` → `%3D`, `/` → `%2F`) so keys can never collide
    /// with an unscoped key or smuggle a delimiter, and pairs are emitted in
    /// sorted key order so the same map always yields the same key. Scoping is
    /// additive: calling this on an already-contextual repository nests the
    /// new segment after the existing one.
    pub fn with_context(&self, context: HashMap<String, String>) -> Repository {
        let mut pairs: Vec<(String, String)> = context
            .into_iter()
            .map(|(k, v)| (k, percent_encode(&v)))
            .collect();
        pairs.sort_unstable();
        let segment = pairs
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        let segment = match self.context {
            Some(ref base) if !base.is_empty() => format!("{base}:{segment}"),
            _ => segment,
        };
        Repository {
            store: self.store.clone(),
            context: Some(segment),
            prefix: self.prefix.clone(),
        }
    }

    /// Store this repository wraps.
    pub fn inner(&self) -> &StoreRef {
        &self.store
    }

    /// Context segment carried by this repository, if any.
    pub fn context(&self) -> Option<&str> {
        self.context.as_deref()
    }

    /// Prefixed key helper.
    fn key(&self, key: &str) -> String {
        match self.context {
            Some(ref ctx) => format!("{}{}:{}", self.prefix, ctx, key),
            None => format!("{}{}", self.prefix, key),
        }
    }

    /// Fetch a typed value, or `None` on miss/expiry.
    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        match self.store.get(&self.key(key)).await? {
            Some(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| CacheError::Serialization(e.to_string())),
            None => Ok(None),
        }
    }

    /// Store a typed value for `ttl` (0 = forever).
    pub async fn put<T: Serialize + Sync>(
        &self,
        key: &str,
        value: &T,
        ttl: Duration,
    ) -> Result<()> {
        let bytes =
            serde_json::to_vec(value).map_err(|e| CacheError::Serialization(e.to_string()))?;
        self.store.put(&self.key(key), bytes, ttl).await
    }

    /// Store `value` only when `key` is absent; `Ok(false)` if it exists.
    ///
    /// Delegates to the store's atomic `put_if_absent` (SET NX EX / map-guard),
    /// so concurrent `add` calls cannot both win a missing key.
    pub async fn add<T: Serialize + Sync>(
        &self,
        key: &str,
        value: &T,
        ttl: Duration,
    ) -> Result<bool> {
        let bytes =
            serde_json::to_vec(value).map_err(|e| CacheError::Serialization(e.to_string()))?;
        self.store.put_if_absent(&self.key(key), bytes, ttl).await
    }

    /// Compute-and-cache: return the cached value or store `producer`'s output.
    pub async fn remember<T, F, Fut>(&self, key: &str, ttl: Duration, producer: F) -> Result<T>
    where
        T: DeserializeOwned + Serialize + Send + Sync + 'static,
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<T>> + Send + 'static,
    {
        if let Some(hit) = self.get::<T>(key).await? {
            return Ok(hit);
        }
        let value = producer().await?;
        self.put(key, &value, ttl).await?;
        Ok(value)
    }

    /// Cache `value` with no expiry.
    pub async fn forever<T: Serialize + Sync>(&self, key: &str, value: &T) -> Result<()> {
        self.put(key, value, Duration::ZERO).await
    }

    /// Fetch and remove a typed value in one step.
    pub async fn pull<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let value = self.get::<T>(key).await?;
        self.store.forget(&self.key(key)).await?;
        Ok(value)
    }

    /// Whether a live value exists under `key`.
    pub async fn has(&self, key: &str) -> Result<bool> {
        Ok(self.store.get(&self.key(key)).await?.is_some())
    }

    /// Extend the TTL of `key` without reading it (`false` when missing).
    pub async fn touch(&self, key: &str, ttl: Duration) -> Result<bool> {
        self.store.touch(&self.key(key), ttl).await
    }

    /// Remove `key`.
    pub async fn forget(&self, key: &str) -> Result<()> {
        self.store.forget(&self.key(key)).await
    }

    /// Remove every key from the underlying store.
    pub async fn flush(&self) -> Result<()> {
        self.store.flush().await
    }

    /// Atomically add `n` to the numeric value under `key`.
    pub async fn increment(&self, key: &str, n: i64) -> Result<i64> {
        self.store.increment(&self.key(key), n).await
    }

    /// Atomically subtract `n` from the numeric value under `key`.
    pub async fn decrement(&self, key: &str, n: i64) -> Result<i64> {
        self.store.decrement(&self.key(key), n).await
    }
}

/// Backwards-compatible trait alias for code written against the doc shape.
///
/// The doc-shape `Repository` facade (remember/forever/pull/has/withContext/
/// store) is satisfied by `CacheManager` + `Repository`; this marker keeps the
/// `Store`+`Repository` naming discoverable.
#[async_trait]
pub trait RepositoryLike: Send + Sync {
    /// Look up a typed value.
    async fn get_typed<T: DeserializeOwned + Send>(&self, key: &str) -> Result<Option<T>>;
}

#[async_trait]
impl RepositoryLike for Repository {
    /// Look up a typed value through the prefixed key.
    async fn get_typed<T: DeserializeOwned + Send>(&self, key: &str) -> Result<Option<T>> {
        self.get::<T>(key).await
    }
}

/// Percent-encode a context value for safe embedding in a cache key segment.
///
/// Only unreserved URI characters survive as-is; `%` is encoded first so
/// double-encoding never produces ambiguity. This guarantees the context
/// segment contains no `:`, `&`, `=` or `/` of its own, keeping composite keys
/// unambiguous and collision-free.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        let allowed = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
        if allowed {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}
