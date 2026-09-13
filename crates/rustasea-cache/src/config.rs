//! Typed `[cache]` configuration — Laravel 13.x `config/cache.php` parity.
//!
//! [`CacheConfig`] mirrors the shape of Laravel's `config/cache.php`: a
//! `default` store selector, a `stores` map of named drivers, a `prefix` used
//! to namespace every key, and a `serializable_classes` allow-list. It is read
//! through [`rustasea_config::ConfigLoader`] via [`CacheConfig::from_loader`],
//! which also validates the configuration fail-closed:
//!
//! * the prefix must contain the hyphenated `-cache-` marker (the same rule
//!   `SessionPolicy` enforces, so cache keys never collide with session keys);
//! * `default` must name a declared store (the `memory` store is implicitly
//!   always available, so a config that omits it still resolves);
//! * every declared store must use a supported driver — `memory` / `array`
//!   (Laravel's `array` maps onto RustaSea's in-process store) or `redis`.
//!
//! The remaining Laravel drivers (`database`, `file`, `storage`, `memcached`,
//! `dynamodb`, `failover`, `octane`) are recognised by name but not yet
//! implemented: selecting one raises
//! [`CacheConfigError::UnsupportedDriver`] at load time rather than silently
//! falling back to a store that loses entries on restart.
use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer};

use rustasea_config::ConfigLoader;

use crate::error::CacheConfigError;
use crate::repository::{MEMORY_STORE, REDIS_STORE};

/// Store selected when `cache.default` is omitted.
pub const DEFAULT_STORE: &str = "memory";

/// Prefix applied to every key when `cache.prefix` is omitted.
///
/// Contains the required `-cache-` marker, mirroring Laravel's
/// `Str::slug(APP_NAME).'-cache-'` default.
pub const DEFAULT_CACHE_PREFIX: &str = "rustasea-cache-";

/// Laravel drivers recognised but not yet implemented by RustaSea.
const UNIMPLEMENTED_DRIVERS: &[&str] = &[
    "database",
    "file",
    "storage",
    "memcached",
    "dynamodb",
    "failover",
    "octane",
];

/// A store driver RustaSea can actually build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheDriver {
    /// In-process store; Laravel's `array` (and RustaSea's `memory`).
    Memory,
    /// Shared `deadpool-redis` store (feature `redis`).
    Redis,
}

/// One `[cache.stores.<name>]` entry (union of every Laravel driver shape).
///
/// A store sets only the fields its driver uses; every field is optional so a
/// Laravel config round-trips unchanged. Unknown fields are tolerated by serde
/// so forward-compatible additions never break parsing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct StoreConfig {
    /// Driver selector (`memory`, `array`, `redis`, …).
    #[serde(default)]
    pub driver: String,
    /// Serialize values before storing (Laravel `serialize`; parity only —
    /// RustaSea always encodes typed values as JSON).
    #[serde(default)]
    pub serialize: bool,
    /// Redis connection name or URL (Laravel `connection`).
    #[serde(default)]
    pub connection: Option<String>,
    /// Redis connection used for locks (Laravel `lock_connection`).
    #[serde(default)]
    pub lock_connection: Option<String>,
    /// Database table (unimplemented `database` driver).
    #[serde(default)]
    pub table: Option<String>,
    /// Lock table (unimplemented `database` driver).
    #[serde(default)]
    pub lock_table: Option<String>,
    /// Filesystem path (unimplemented `file` driver).
    #[serde(default)]
    pub path: Option<String>,
    /// Lock filesystem path (unimplemented `file` driver).
    #[serde(default)]
    pub lock_path: Option<String>,
    /// Storage disk name (unimplemented `storage` driver).
    #[serde(default)]
    pub disk: Option<String>,
    /// Explicit Redis URL; wins over `connection` and `REDIS_URL`.
    #[serde(default)]
    pub url: Option<String>,
    /// Member store names (unimplemented `failover` driver).
    #[serde(default)]
    pub stores: Vec<String>,
    /// Memcached servers (unimplemented `memcached` driver).
    #[serde(default)]
    pub servers: Vec<String>,
    /// DynamoDB endpoint (unimplemented `dynamodb` driver).
    #[serde(default)]
    pub endpoint: Option<String>,
}

impl StoreConfig {
    /// Classify the declared driver into a buildable [`CacheDriver`].
    ///
    /// # Errors
    ///
    /// [`CacheConfigError::UnsupportedDriver`] for a recognised-but-unimplemented
    /// Laravel driver, or [`CacheConfigError::UnknownDriver`] for anything else.
    pub fn driver_kind(&self) -> Result<CacheDriver, CacheConfigError> {
        let driver = self.driver.trim().to_ascii_lowercase();
        if driver == "memory" || driver == "array" {
            Ok(CacheDriver::Memory)
        } else if driver == REDIS_STORE {
            Ok(CacheDriver::Redis)
        } else if UNIMPLEMENTED_DRIVERS.contains(&driver.as_str()) {
            Err(CacheConfigError::UnsupportedDriver(self.driver.clone()))
        } else {
            Err(CacheConfigError::UnknownDriver(self.driver.clone()))
        }
    }

    /// Resolve the Redis URL for this store.
    ///
    /// Precedence: an explicit `url`, then a `connection` that already looks
    /// like a URL (`redis://` / `rediss://`), then the `REDIS_URL` environment
    /// variable. Returns `None` when none is configured.
    pub fn resolved_url(&self) -> Option<String> {
        if let Some(url) = trimmed(self.url.as_deref()) {
            return Some(url.to_string());
        }
        if let Some(connection) = trimmed(self.connection.as_deref()) {
            if connection.starts_with("redis://") || connection.starts_with("rediss://") {
                return Some(connection.to_string());
            }
        }
        trimmed(std::env::var("REDIS_URL").ok().as_deref()).map(str::to_string)
    }
}

/// Trim a string, treating a blank value as absent.
fn trimmed(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

/// Read an environment variable, treating unset or blank values as absent.
fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// Raw `serializable_classes` shape: Laravel's `false` or a class list.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SerializableClassesRaw {
    /// `false` (or `true`) disables the allow-list.
    Flag(#[allow(dead_code)] bool),
    /// An explicit class allow-list.
    List(Vec<String>),
}

/// Deserialize `serializable_classes` accepting both Laravel shapes.
fn de_serializable_classes<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<SerializableClassesRaw>::deserialize(deserializer)?;
    Ok(match raw {
        None | Some(SerializableClassesRaw::Flag(_)) => Vec::new(),
        Some(SerializableClassesRaw::List(list)) => list,
    })
}

/// Typed `[cache]` table (Laravel 13.x `config/cache.php` shape).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CacheConfig {
    /// Default store name (`cache.default`).
    #[serde(default = "default_store_name")]
    pub default: String,
    /// Key prefix applied to every cache key (`cache.prefix`).
    #[serde(default = "default_prefix")]
    pub prefix: String,
    /// Class allow-list (`cache.serializable_classes`; `false` → empty).
    #[serde(default, deserialize_with = "de_serializable_classes")]
    pub serializable_classes: Vec<String>,
    /// Named stores, ordered for deterministic iteration.
    #[serde(default)]
    pub stores: BTreeMap<String, StoreConfig>,
}

/// Default store name (`memory`).
fn default_store_name() -> String {
    DEFAULT_STORE.to_string()
}

/// Default key prefix (`rustasea-cache-`).
fn default_prefix() -> String {
    DEFAULT_CACHE_PREFIX.to_string()
}

impl Default for CacheConfig {
    /// RustaSea defaults: `memory` default store, `rustasea-cache-` prefix.
    fn default() -> Self {
        let mut stores = BTreeMap::new();
        stores.insert(
            MEMORY_STORE.to_string(),
            StoreConfig {
                driver: MEMORY_STORE.to_string(),
                ..StoreConfig::default()
            },
        );
        Self {
            default: default_store_name(),
            prefix: default_prefix(),
            serializable_classes: Vec::new(),
            stores,
        }
    }
}

impl CacheConfig {
    /// Deserialize `[cache]` from a layered [`ConfigLoader`] and validate it.
    ///
    /// A missing `[cache]` table yields [`CacheConfig::default`] (tolerated,
    /// matching the loader's missing-file policy); a table that exists but does
    /// not deserialize surfaces [`CacheConfigError::Invalid`]. The parsed
    /// config then applies the documented `CACHE_*` environment overrides via
    /// [`CacheConfig::apply_env`] and is validated by [`CacheConfig::validate`].
    ///
    /// # Errors
    ///
    /// [`CacheConfigError::Invalid`] on malformed TOML, or any validation error
    /// from [`CacheConfig::validate`].
    pub fn from_loader(loader: &ConfigLoader) -> Result<Self, CacheConfigError> {
        let mut config = match loader.get_key::<CacheConfig>("cache") {
            Ok(config) => config,
            Err(error) => {
                if loader.inner().get_table("cache").is_err() {
                    CacheConfig::default()
                } else {
                    return Err(CacheConfigError::Invalid(error.to_string()));
                }
            }
        };
        config.apply_env();
        config.validate()?;
        Ok(config)
    }

    /// Apply the documented single-underscore `CACHE_*` environment overrides.
    ///
    /// `CACHE_PREFIX` replaces the key prefix (`cache.prefix`); the environment
    /// wins over the file and a blank value is ignored. The loader's `__`
    /// separator means single-underscore variables never reach the nested
    /// `[cache]` table, so this bridge is the only path for `.env` parity.
    pub fn apply_env(&mut self) {
        if let Some(prefix) = env_non_empty("CACHE_PREFIX") {
            self.prefix = prefix;
        }
    }

    /// Validate the prefix, the default selector, and every declared driver.
    ///
    /// The `memory` store is implicitly available (it is always registered by
    /// the manager), so `default = "memory"` is valid even when the store is
    /// not explicitly declared.
    ///
    /// # Errors
    ///
    /// [`CacheConfigError::InvalidPrefix`] when the prefix lacks `-cache-`,
    /// [`CacheConfigError::UnknownDefaultStore`] when `default` names an
    /// undeclared store, or a driver error from [`StoreConfig::driver_kind`].
    pub fn validate(&self) -> Result<(), CacheConfigError> {
        if !self.prefix.contains("-cache-") {
            return Err(CacheConfigError::InvalidPrefix(self.prefix.clone()));
        }
        if !self.stores.contains_key(&self.default) && self.default != MEMORY_STORE {
            return Err(CacheConfigError::UnknownDefaultStore(self.default.clone()));
        }
        for store in self.stores.values() {
            store.driver_kind()?;
        }
        Ok(())
    }

    /// Name of the default store (`cache.default`).
    pub fn default_store(&self) -> &str {
        &self.default
    }

    /// The configured key prefix (`cache.prefix`).
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Look up a declared store by name.
    pub fn store_config(&self, name: &str) -> Option<&StoreConfig> {
        self.stores.get(name)
    }
}

#[cfg(test)]
mod tests;
