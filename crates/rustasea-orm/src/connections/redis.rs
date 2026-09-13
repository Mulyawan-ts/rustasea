//! Redis-section config for the `[database]` table.
//!
//! Split out of [`crate::connections`] to keep the parent module within the
//! file-size limit. [`RedisConfig`] mirrors Laravel's `database.php` `redis`
//! block: a `client` selector, a shared `options` table, and a named map of
//! connection *definitions* the cache/queue configs reference by name
//! (`default`, `cache`). The ORM only parses and validates these entries; it
//! never opens a Redis socket itself, so no Redis driver is linked.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::error::{ConnectionError, OrmError, Result};

/// Redis URL schemes accepted by [`RedisConnection::validate`].
const REDIS_SCHEMES: [&str; 3] = ["redis", "rediss", "unix"];

/// Laravel-parity `[database.redis]` section.
///
/// `client` selects the backend (RustaSea uses `deadpool`). `options` holds the
/// shared connection options, and every other key is a named
/// [`RedisConnection`] captured by the flattened `connections` map — so
/// `[database.redis.default]` and `[database.redis.cache]` land here by name.
#[derive(Debug, Clone, Deserialize)]
pub struct RedisConfig {
    /// Backend selector (`deadpool` by default; Laravel `phpredis`/`predis`).
    #[serde(default = "default_redis_client")]
    pub client: String,
    /// Shared options applied to every Redis connection.
    #[serde(default)]
    pub options: RedisOptions,
    /// Named connection definitions, ordered by name for deterministic output.
    #[serde(flatten)]
    pub connections: BTreeMap<String, RedisConnection>,
}

impl Default for RedisConfig {
    /// Empty connection map with the default client and options.
    fn default() -> Self {
        Self {
            client: default_redis_client(),
            options: RedisOptions::default(),
            connections: BTreeMap::new(),
        }
    }
}

impl RedisConfig {
    /// Default Redis backend selector when `client` is omitted.
    pub fn client(&self) -> &str {
        self.client.as_str()
    }

    /// Look up a named connection definition (`default`, `cache`, …).
    pub fn connection(&self, name: &str) -> Option<&RedisConnection> {
        self.connections.get(name)
    }

    /// Validate every named connection definition.
    ///
    /// # Errors
    ///
    /// [`ConnectionError::InvalidConfig`] naming the offending connection when
    /// its `url` carries an unsupported scheme.
    pub fn validate(&self) -> Result<()> {
        for (name, connection) in &self.connections {
            if let Some(detail) = connection.url_scheme_error() {
                return Err(OrmError::Connection(ConnectionError::InvalidConfig(
                    format!("redis connection `{name}`: {detail}"),
                )));
            }
        }
        Ok(())
    }
}

/// Shared `[database.redis.options]` block.
///
/// Mirrors Laravel's `options`: `cluster` names the cluster backend, `prefix`
/// is prepended to every key, and `persistent` keeps connections open across
/// requests. All three are parsed for parity; RustaSea applies them where the
/// consuming cache/queue driver supports the equivalent setting.
#[derive(Debug, Clone, Deserialize)]
pub struct RedisOptions {
    /// Cluster backend name (Laravel `cluster`, default `redis`).
    #[serde(default = "default_redis_cluster")]
    pub cluster: String,
    /// Key prefix applied to every Redis key (Laravel `prefix`).
    #[serde(default)]
    pub prefix: String,
    /// Whether to reuse connections across requests (Laravel `persistent`).
    #[serde(default)]
    pub persistent: bool,
}

impl Default for RedisOptions {
    /// Laravel defaults: cluster `redis`, empty prefix, non-persistent.
    fn default() -> Self {
        Self {
            cluster: default_redis_cluster(),
            prefix: String::new(),
            persistent: false,
        }
    }
}

/// One named Redis connection definition (`[database.redis.<name>]`).
///
/// A full `url` (whose scheme must be `redis://`, `rediss://`, or `unix://`)
/// takes precedence over the granular `host`/`port`/`database` fields.
/// `max_retries` and the `backoff_*` keys describe the reconnect policy: the
/// algorithm name, the base delay in milliseconds, and the delay cap.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RedisConnection {
    /// Full connection URL; overrides the granular fields when present.
    #[serde(default)]
    pub url: Option<String>,
    /// Host name or IP address (granular form).
    #[serde(default)]
    pub host: Option<String>,
    /// TCP port (granular form; Redis default `6379`).
    #[serde(default)]
    pub port: Option<u16>,
    /// Login user (Redis 6 ACL; Laravel `REDIS_USERNAME`).
    #[serde(default)]
    pub username: Option<String>,
    /// Login password (Laravel `REDIS_PASSWORD`).
    #[serde(default)]
    pub password: Option<String>,
    /// Logical Redis database index (default `0`; the `cache` connection uses `1`).
    #[serde(default)]
    pub database: Option<u32>,
    /// Number of reconnect attempts before giving up.
    #[serde(default)]
    pub max_retries: Option<u32>,
    /// Backoff algorithm name (`exponential`, `constant`, …).
    #[serde(default)]
    pub backoff_algorithm: Option<String>,
    /// Backoff base delay in milliseconds.
    #[serde(default)]
    pub backoff_base: Option<u64>,
    /// Backoff maximum delay in milliseconds.
    #[serde(default)]
    pub backoff_cap: Option<u64>,
}

impl RedisConnection {
    /// Describe an unsupported `url` scheme, when one is present.
    ///
    /// Returns `None` for an absent/blank URL (the granular fields then apply).
    fn url_scheme_error(&self) -> Option<String> {
        let url = self.url.as_deref()?.trim();
        if url.is_empty() {
            return None;
        }
        let scheme = url
            .split(':')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if REDIS_SCHEMES.contains(&scheme.as_str()) {
            return None;
        }
        Some(format!(
            "URL `{url}` has unsupported scheme `{scheme}` (expected redis://, rediss://, or unix://)"
        ))
    }

    /// Validate this connection definition in isolation.
    ///
    /// # Errors
    ///
    /// [`ConnectionError::InvalidConfig`] when a declared `url` does not use a
    /// recognised Redis scheme.
    pub fn validate(&self) -> Result<()> {
        match self.url_scheme_error() {
            Some(detail) => Err(OrmError::Connection(ConnectionError::InvalidConfig(detail))),
            None => Ok(()),
        }
    }
}

/// Default Redis backend selector (RustaSea's pooled `deadpool` client).
fn default_redis_client() -> String {
    "deadpool".to_string()
}

/// Default cluster backend name (Laravel `redis`).
fn default_redis_cluster() -> String {
    "redis".to_string()
}
