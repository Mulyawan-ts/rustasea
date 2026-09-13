//! Config-driven named database connections and lazy pool resolution.
//!
//! This module owns parsing of the `[database]` config table (read through
//! [`rustasea_config::ConfigLoader`]) and turns it into a Laravel-style map of
//! named connections plus a [`ConnectionResolver`] that opens and caches one
//! [`ConnectionPair`] per name on demand.
//!
//! Two config shapes are accepted and may coexist:
//!
//! 1. **Legacy flat** — a single `database.url` (with an optional `driver`
//!    hint). When no `[database.connections]` table is present the resolver
//!    synthesizes one implicit connection named `default` so old configs keep
//!    working unchanged.
//! 2. **Named connections** — `[database.connections.<name>]` entries declaring
//!    a `driver` plus either a full `url` or granular
//!    `host`/`port`/`database`/`username`/`password`/`charset` fields. The
//!    `default` key selects the connection used when no name is given.
//!
//! ## Read/write splitting
//!
//! A named connection may declare optional `[….read]` / `[….write]` endpoint
//! tables (each a full `url` or granular fields). Every field of an endpoint
//! inherits the primary value when omitted, so a replica sharing credentials
//! only needs its own `host`:
//!
//! ```toml
//! [database.connections.pgsql]
//! driver = "postgres"
//! host = "127.0.0.1"
//! database = "app"
//!
//! [database.connections.pgsql.read]
//! host = "replica.internal"
//! ```
//!
//! [`ConnectionResolver::pair`] returns a [`ConnectionPair`] whose
//! `read`/`write` pools are opened and cached independently. Absent any split
//! both roles share the *same* pool instance (identity preserved), so a
//! non-split app behaves exactly as before. [`ConnectionResolver::resolve`]
//! keeps returning the write (primary) pool for backward compatibility.
//!
//! The `driver` field is both a selector and a validator: it is cross-checked
//! against the URL scheme and a disagreement surfaces as a typed
//! [`ConnectionError::DriverMismatch`].
//!
//! ## Migrations and Redis sections
//!
//! The Laravel `database.php` `migrations` and `redis` blocks are parsed too:
//! `[database.migrations]` becomes [`MigrationsConfig`] (the tracking table the
//! [`crate::Migrator`] records into) and `[database.redis]` becomes an optional
//! [`RedisConfig`] whose named connection definitions (`default`, `cache`) the
//! cache/queue configs reference by name. Both are optional; absent sections
//! fall back to Laravel defaults and never fail parsing.
//!
//! Environment-variable precedence is intentionally *not* handled here — the
//! CLI and `xtask` keep `DATABASE_URL`/`DATABASE__URL` ahead of this module so a
//! single-URL app behaves exactly as before.

use std::collections::{BTreeMap, HashMap};

use serde::Deserialize;
use tokio::sync::Mutex;

use rustasea_config::ConfigLoader;

use crate::db::{DbPool, PoolSettings};
use crate::error::{ConnectionError, OrmError, Result};

mod migrations;
mod mongo;
mod pair;
mod pool;
mod redis;

pub use migrations::MigrationsConfig;
pub use mongo::DatabaseConnection;
pub use pair::ConnectionPair;
pub use pool::{EndpointConfig, PoolConfig};
pub use redis::{RedisConfig, RedisConnection, RedisOptions};

/// Name assigned to the implicit connection synthesized from a legacy `url`.
const IMPLICIT_DEFAULT_NAME: &str = "default";

/// One named database connection.
///
/// `driver` is required and selects/validates the engine. A connection supplies
/// either a complete `url` or the granular fields the driver needs; when both
/// are present the `url` wins but is still cross-checked against `driver`.
///
/// For `driver = "mongodb"` the connection string may be given as `uri`
/// (aliased to `url`); see [`crate::connections::ConnectionConfig::build_mongo_uri`].
///
/// Optional `[….read]` / `[….write]` tables split the connection into a
/// Laravel-style read/write pair; each overlays the primary fields and falls
/// back to them when omitted. Absent both, reads and writes share one pool.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConnectionConfig {
    /// Driver selector: `sqlite` | `postgres` | `mysql` | `mongodb` (aliases accepted).
    pub driver: String,
    /// Full connection URL (or `uri`); when set it takes precedence over
    /// granular fields. The `uri` key is accepted as an alias for MongoDB.
    #[serde(default, alias = "uri")]
    pub url: Option<String>,
    /// Host name or IP address (network drivers).
    #[serde(default)]
    pub host: Option<String>,
    /// TCP port (network drivers); defaults per driver when omitted.
    #[serde(default)]
    pub port: Option<u16>,
    /// Database name (network drivers) or file path (`sqlite`).
    #[serde(default)]
    pub database: Option<String>,
    /// Login user (network drivers).
    #[serde(default)]
    pub username: Option<String>,
    /// Login password (network drivers).
    #[serde(default)]
    pub password: Option<String>,
    /// Optional client charset appended as `?charset=…` (network drivers).
    #[serde(default)]
    pub charset: Option<String>,
    /// Optional read-replica endpoint; absent → reads use the write endpoint.
    #[serde(default)]
    pub read: Option<EndpointConfig>,
    /// Optional write (primary) endpoint; absent → writes use the primary fields.
    #[serde(default)]
    pub write: Option<EndpointConfig>,
    /// Per-connection pool overrides; falls back to the `[database.pool]` table.
    #[serde(default)]
    pub pool: Option<PoolConfig>,
}

/// Typed `[database]` table: a default selector plus named connections.
///
/// The legacy flat `url` (and its `driver` hint) are retained so a single-URL
/// config keeps working; [`DatabaseConfig::synthesize`] folds them into an
/// implicit connection when no explicit `[database.connections]` exist.
///
/// The Laravel-parity `migrations` and `redis` sections are parsed alongside the
/// connections: [`MigrationsConfig`] names the tracking table and
/// [`RedisConfig`] holds the cache/queue connection definitions by name. Both
/// are optional and default when absent, so a connections-only config keeps
/// parsing unchanged.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DatabaseConfig {
    /// Name of the connection used when `resolve(None)` is called.
    #[serde(default)]
    pub default: Option<String>,
    /// Legacy driver hint for the flat `url` (consumed by the implicit connection).
    #[serde(default)]
    pub driver: Option<String>,
    /// Legacy flat connection URL.
    #[serde(default)]
    pub url: Option<String>,
    /// Default pool tuning shared by every connection lacking its own `pool`.
    #[serde(default)]
    pub pool: Option<PoolConfig>,
    /// Named connections, ordered by name for deterministic iteration.
    #[serde(default)]
    pub connections: BTreeMap<String, ConnectionConfig>,
    /// Migration tracking-table settings (`[database.migrations]`).
    #[serde(default)]
    pub migrations: MigrationsConfig,
    /// Redis connection definitions (`[database.redis]`); absent when unset.
    #[serde(default)]
    pub redis: Option<RedisConfig>,
}

impl DatabaseConfig {
    /// Deserialize `[database]` from a layered [`ConfigLoader`].
    ///
    /// A missing `[database]` table yields an empty config (tolerated, matching
    /// the loader's missing-file policy); a malformed table surfaces a typed
    /// [`ConnectionError::InvalidConfig`]. Legacy flat fields are folded into an
    /// implicit connection by [`DatabaseConfig::synthesize`]. When a
    /// `[database.redis]` section is present its connection definitions are
    /// validated (unsupported URL scheme → typed error).
    ///
    /// # Errors
    ///
    /// Returns [`OrmError::Connection`] when the `[database]` table exists but
    /// cannot be deserialized, or when a Redis connection carries an invalid
    /// `url` scheme.
    pub fn from_loader(loader: &ConfigLoader) -> Result<Self> {
        let mut config = match loader.get_key::<DatabaseConfig>("database") {
            Ok(config) => config,
            Err(error) => {
                // A missing `[database]` table is tolerated (the loader treats a
                // missing `config/` directory the same way); a table that exists
                // but does not deserialize is a genuine configuration error.
                if loader.inner().get_table("database").is_err() {
                    DatabaseConfig::default()
                } else {
                    return Err(OrmError::Connection(ConnectionError::InvalidConfig(
                        error.to_string(),
                    )));
                }
            }
        };
        if let Some(redis) = &config.redis {
            redis.validate()?;
        }
        config.synthesize();
        Ok(config)
    }

    /// Name of the migration tracking table (default `migrations`).
    ///
    /// Read from `[database.migrations] table`; a config without that section
    /// yields the Laravel default.
    pub fn migrations_table(&self) -> &str {
        self.migrations.table.as_str()
    }

    /// Whether a re-published migration refreshes its recorded date.
    pub fn update_date_on_publish(&self) -> bool {
        self.migrations.update_date_on_publish
    }

    /// The named Redis connection definition, when a `[database.redis]` section
    /// declares one under `name` (`default`, `cache`, …).
    pub fn redis_connection(&self, name: &str) -> Option<&RedisConnection> {
        self.redis.as_ref().and_then(|redis| redis.connection(name))
    }

    /// Fold legacy flat fields into the connection map.
    ///
    /// When `connections` is empty and a legacy `url` is set, a single implicit
    /// connection (named by `default`, else `default`) is inserted. When named
    /// connections exist but no `default` is declared, a lone connection becomes
    /// the default; multiple connections without a `default` stay unresolved.
    pub fn synthesize(&mut self) {
        if self.connections.is_empty() {
            let Some(url) = self.url.clone().filter(|url| !url.trim().is_empty()) else {
                return;
            };
            let driver = self
                .driver
                .clone()
                .filter(|driver| !driver.trim().is_empty())
                .unwrap_or_else(|| scheme_of(&url).to_string());
            let name = self
                .default
                .clone()
                .unwrap_or_else(|| IMPLICIT_DEFAULT_NAME.to_string());
            self.connections.insert(
                name.clone(),
                ConnectionConfig {
                    driver,
                    url: Some(url),
                    pool: self.pool.clone(),
                    ..ConnectionConfig::default()
                },
            );
            self.default = Some(name);
            return;
        }

        if self.default.is_none() && self.connections.len() == 1 {
            self.default = self.connections.keys().next().cloned();
        }
    }

    /// The effective default connection name, when one can be determined.
    pub fn default_name(&self) -> Option<&str> {
        self.default.as_deref()
    }

    /// Build the URL for `name`, or for the default connection when `None`.
    ///
    /// Returns the write (primary) URL, i.e. the `[….write]` overlay when one is
    /// declared, else the primary fields. For a `driver = "mongodb"` connection
    /// it returns the assembled `mongodb://` URI
    /// ([`ConnectionConfig::build_mongo_uri`]). This validates the connection
    /// without opening it, which is what the CLI and `xtask` need to keep their
    /// single-URL behaviour.
    ///
    /// # Errors
    ///
    /// [`ConnectionError::NotConfigured`] when no default exists,
    /// [`ConnectionError::UnknownConnection`] for an undeclared name, or any
    /// [`ConnectionConfig::build_url`] / [`ConnectionConfig::build_mongo_uri`]
    /// error.
    pub fn resolve_url(&self, name: Option<&str>) -> Result<String> {
        let name = match name {
            Some(name) => name,
            None => self
                .default_name()
                .ok_or(OrmError::Connection(ConnectionError::NotConfigured))?,
        };
        let connection = self.connections.get(name).ok_or_else(|| {
            OrmError::Connection(ConnectionError::UnknownConnection(name.to_string()))
        })?;
        if connection.is_mongo() {
            return connection.build_mongo_uri();
        }
        connection.build_url()
    }

    /// Effective pool settings for `name`, preferring a per-connection override.
    fn pool_settings(&self, name: &str) -> PoolSettings {
        self.connections
            .get(name)
            .and_then(|connection| connection.pool.as_ref())
            .or(self.pool.as_ref())
            .map(PoolConfig::to_settings)
            .unwrap_or_default()
    }
}

/// Lazily connects and caches one read/write [`ConnectionPair`] per name.
///
/// The resolver is cheap to construct and safe to share (e.g. inside an
/// `AppState`); each resolution returns a clone of the cached pair, opening the
/// connection only on first use. When a connection declares no read/write
/// split, its pair holds the *same* pool for both roles (identity preserved).
///
/// With the `mongodb` feature, `driver = "mongodb"` connections are cached
/// separately and returned by [`ConnectionResolver::resolve_any`] as
/// [`DatabaseConnection::Mongo`].
#[derive(Debug)]
pub struct ConnectionResolver {
    config: DatabaseConfig,
    cache: Mutex<HashMap<String, ConnectionPair>>,
    #[cfg(feature = "mongodb")]
    mongo_cache: Mutex<HashMap<String, rustasea_mongo::MongoClient>>,
}

impl ConnectionResolver {
    /// Create a resolver over an already-parsed [`DatabaseConfig`].
    pub fn new(config: DatabaseConfig) -> Self {
        Self {
            config,
            cache: Mutex::new(HashMap::new()),
            #[cfg(feature = "mongodb")]
            mongo_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Build a resolver from a layered [`ConfigLoader`].
    ///
    /// # Errors
    ///
    /// Propagates [`DatabaseConfig::from_loader`] failures.
    pub fn from_loader(loader: &ConfigLoader) -> Result<Self> {
        Ok(Self::new(DatabaseConfig::from_loader(loader)?))
    }

    /// Borrow the underlying configuration.
    pub fn config(&self) -> &DatabaseConfig {
        &self.config
    }

    /// Names of the connections currently held in the cache.
    pub async fn cached_names(&self) -> Vec<String> {
        self.cache.lock().await.keys().cloned().collect()
    }

    /// Resolve `name` (or the configured default when `None`) to a live pool.
    ///
    /// Kept for backward compatibility: returns the write (primary) pool of the
    /// resolved pair, which is also the read pool when no split is configured.
    /// New code that routes reads/writes should prefer [`ConnectionResolver::pair`].
    ///
    /// # Errors
    ///
    /// [`ConnectionError::NotConfigured`] when no default exists,
    /// [`ConnectionError::UnknownConnection`] for an undeclared name, or a
    /// typed build/connect failure from [`ConnectionConfig::build_url`] /
    /// [`DbPool::connect_with_settings`].
    pub async fn resolve(&self, name: Option<&str>) -> Result<DbPool> {
        Ok(self.pair(name).await?.into_write())
    }

    /// Resolve the read pool for `name` (or the default), connecting lazily.
    ///
    /// # Errors
    ///
    /// Propagates [`ConnectionResolver::pair`] failures.
    pub async fn read(&self, name: Option<&str>) -> Result<DbPool> {
        Ok(self.pair(name).await?.into_read())
    }

    /// Resolve the write pool for `name` (or the default), connecting lazily.
    ///
    /// # Errors
    ///
    /// Propagates [`ConnectionResolver::pair`] failures.
    pub async fn write(&self, name: Option<&str>) -> Result<DbPool> {
        Ok(self.pair(name).await?.into_write())
    }

    /// Resolve the effective connection name: the given one, else the default.
    ///
    /// Shared by [`ConnectionResolver::pair`] and
    /// [`ConnectionResolver::resolve_any`] so both apply the same
    /// `NotConfigured` rule when no default is declared.
    ///
    /// # Errors
    ///
    /// [`ConnectionError::NotConfigured`] when `name` is `None` and no default
    /// connection is configured.
    pub(crate) fn resolve_name(&self, name: Option<&str>) -> Result<String> {
        match name {
            Some(name) => Ok(name.to_string()),
            None => self
                .config
                .default_name()
                .map(str::to_string)
                .ok_or(OrmError::Connection(ConnectionError::NotConfigured)),
        }
    }

    /// Resolve `name` to a cached read/write [`ConnectionPair`].
    ///
    /// The first call for a name builds the write (and, when a split is
    /// configured, the read) URL, applies the connection's pool settings,
    /// connects, and caches the pair; subsequent calls return the cached pair.
    /// A connection without a read overlay reuses one pool instance for both
    /// roles. SQLite in-memory URLs remain pinned to a single connection.
    ///
    /// This method is SQL-only: a `driver = "mongodb"` connection yields a typed
    /// [`ConnectionError::UnsupportedDriver`]. Use
    /// [`ConnectionResolver::resolve_any`] to resolve MongoDB connections.
    ///
    /// # Errors
    ///
    /// [`ConnectionError::NotConfigured`] when no default exists,
    /// [`ConnectionError::UnknownConnection`] for an undeclared name, a typed
    /// [`ConnectionError`] from URL assembly (invalid read/write URL or missing
    /// field), or [`OrmError::Pool`] when an endpoint cannot be reached.
    pub async fn pair(&self, name: Option<&str>) -> Result<ConnectionPair> {
        let name = self.resolve_name(name)?;

        let connection = self.config.connections.get(&name).ok_or_else(|| {
            OrmError::Connection(ConnectionError::UnknownConnection(name.clone()))
        })?;
        if connection.is_mongo() {
            return Err(OrmError::Connection(ConnectionError::UnsupportedDriver {
                driver: "mongodb (SQL pool requested; use resolve_any)".to_string(),
            }));
        }

        let mut cache = self.cache.lock().await;
        if let Some(pair) = cache.get(&name) {
            return Ok(pair.clone());
        }

        let write_url = connection.build_url()?;
        let read_url = connection.read_url()?;
        let settings = self.config.pool_settings(&name);

        let write = DbPool::connect_with_settings(&write_url, settings).await?;
        let pair = if read_url == write_url {
            // No split (or identical endpoints): one pool serves both roles.
            ConnectionPair::new(write.clone(), write)
        } else {
            let read = DbPool::connect_with_settings(&read_url, settings).await?;
            ConnectionPair::new(read, write)
        };
        cache.insert(name, pair.clone());
        Ok(pair)
    }
}

/// Extract the scheme from a URL (`sqlite://…` → `sqlite`).
fn scheme_of(url: &str) -> &str {
    url.split(':').next().unwrap_or_default()
}

#[cfg(test)]
mod tests;
