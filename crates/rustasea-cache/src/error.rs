/// Typed errors for the cache layer.
use thiserror::Error;

/// Alias for results produced by cache operations.
pub type Result<T> = std::result::Result<T, CacheError>;

/// Top-level cache error type (api-cache.md §4 catalogue).
#[derive(Debug, Error)]
pub enum CacheError {
    /// The backing store (Redis/Memory) is unreachable.
    #[error("cache store unavailable: {0}")]
    StoreUnavailable(String),

    /// A TTL-extension (`touch`) failed at the store level.
    #[error("cache touch failed: {0}")]
    TouchFailed(String),

    /// A value could not be serialized/deserialized.
    #[error("cache serialization failed: {0}")]
    Serialization(String),

    /// No store is registered under the requested name.
    #[error("unknown cache store: {0}")]
    UnknownStore(String),

    /// The `[cache]` configuration is invalid (bad prefix, unknown default, …).
    #[error("cache configuration error: {0}")]
    Config(String),
}

/// Typed `[cache]` configuration errors (Laravel 13.x `config/cache.php` parity).
///
/// Parsing / wiring failures are expressed as stable variants so callers match
/// on types instead of strings; [`CacheConfigError::code`] returns the
/// machine-readable code used in logs and tests. A `CacheConfigError` promotes
/// into [`CacheError::Config`] via [`From`] so configuration problems flow
/// through the same error space as runtime cache failures.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CacheConfigError {
    /// The `[cache]` table exists but cannot be deserialized.
    #[error("invalid cache configuration: {0}")]
    Invalid(String),

    /// `cache.default` names a store that is not declared under `[cache.stores]`.
    #[error("unknown default cache store {0:?}")]
    UnknownDefaultStore(String),

    /// A store declares a driver RustaSea does not recognise at all.
    #[error("unknown cache driver {0:?}")]
    UnknownDriver(String),

    /// A store declares a recognised-but-unimplemented driver (`database`,
    /// `file`, `storage`, `memcached`, `dynamodb`, `failover`, `octane`).
    #[error("cache driver {0:?} is not supported by rustasea-cache")]
    UnsupportedDriver(String),

    /// The configured key prefix is missing the hyphenated `-cache-` marker.
    #[error("cache prefix {0:?} must contain the -cache- marker")]
    InvalidPrefix(String),

    /// A `redis` store is declared but no URL/connection is configured, or the
    /// crate was built without the `redis` feature.
    #[error("cache store {store:?} requires the `redis` feature and a configured url")]
    RedisUnavailable {
        /// Name of the store that could not be wired.
        store: String,
    },
}

impl CacheConfigError {
    /// Stable machine-readable code, e.g. `CacheConfigError::UnsupportedDriver`.
    pub fn code(&self) -> String {
        let variant = match self {
            CacheConfigError::Invalid(_) => "Invalid",
            CacheConfigError::UnknownDefaultStore(_) => "UnknownDefaultStore",
            CacheConfigError::UnknownDriver(_) => "UnknownDriver",
            CacheConfigError::UnsupportedDriver(_) => "UnsupportedDriver",
            CacheConfigError::InvalidPrefix(_) => "InvalidPrefix",
            CacheConfigError::RedisUnavailable { .. } => "RedisUnavailable",
        };
        format!("CacheConfigError::{variant}")
    }
}

impl From<CacheConfigError> for CacheError {
    /// Promote a configuration failure into the cache error space.
    fn from(e: CacheConfigError) -> Self {
        CacheError::Config(e.to_string())
    }
}

/// Lock-specific error surfaced by `Lock::block` timeouts.
#[derive(Debug, Error)]
pub enum LockError {
    /// The lock is still held when the wait window expires.
    #[error("lock contention: {key} already held after {waited:?}")]
    AlreadyHeld {
        /// Lock key that stayed held.
        key: String,
        /// Total time waited before giving up.
        waited: std::time::Duration,
    },

    /// The underlying store failed while acquiring/releasing the lock.
    #[error("lock store unavailable: {0}")]
    StoreUnavailable(String),

    /// The lease expired while blocked; the holder may still be running.
    #[error("lock lease expired for {0}")]
    LeaseExpired(String),
}

impl From<LockError> for CacheError {
    /// Promote a lock failure into the cache error space.
    fn from(e: LockError) -> Self {
        match e {
            LockError::AlreadyHeld { key, waited } => {
                CacheError::TouchFailed(format!("lock {key} held after {waited:?}"))
            }
            LockError::StoreUnavailable(m) => CacheError::StoreUnavailable(m),
            LockError::LeaseExpired(k) => CacheError::TouchFailed(format!("lease expired: {k}")),
        }
    }
}
