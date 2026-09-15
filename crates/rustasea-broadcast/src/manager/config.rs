//! `[broadcasting]` config parsing and the environment bridge (ADOPT-022).
//!
//! [`BroadcastingConfig`] mirrors Laravel's `config/broadcasting.php`: a
//! `default` selector plus a `connections` map. It is deserialized from the
//! layered [`ConfigLoader`] and then overlaid with the documented environment
//! variables, because the loader's `__` separator means single-underscore
//! variables (e.g. `PUSHER_APP_KEY`) never reach the nested table.

use serde::Deserialize;

use rustasea_config::ConfigLoader;

use crate::error::{BroadcastError, Result};

use super::DEFAULT_CONNECTION;

#[cfg(feature = "pusher")]
use crate::pusher::PusherConfig;
#[cfg(feature = "redis")]
use crate::redis_driver::RedisConfig;

/// Read an environment variable, treating unset or blank values as absent.
fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// Typed `[broadcasting]` configuration (Laravel `config/broadcasting.php`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BroadcastingConfig {
    /// Connection used when a publish names none.
    pub default: String,
    /// Pusher driver configuration, when the `pusher` feature is compiled in.
    #[cfg(feature = "pusher")]
    pub pusher: Option<PusherConfig>,
    /// Redis driver configuration, when the `redis` feature is compiled in.
    #[cfg(feature = "redis")]
    pub redis: Option<RedisConfig>,
}

impl Default for BroadcastingConfig {
    /// The shipped default: the in-process `hub` connection, no external
    /// drivers.
    fn default() -> Self {
        Self {
            default: DEFAULT_CONNECTION.to_string(),
            #[cfg(feature = "pusher")]
            pusher: None,
            #[cfg(feature = "redis")]
            redis: None,
        }
    }
}

impl BroadcastingConfig {
    /// Deserialize `[broadcasting]` from a layered [`ConfigLoader`].
    ///
    /// A missing table yields [`BroadcastingConfig::default`] (the `hub`
    /// connection, no external drivers) — matching the loader's missing-file
    /// policy. A table that exists but cannot be deserialized surfaces
    /// [`BroadcastError::Serialization`]. The environment bridge
    /// ([`BroadcastingConfig::apply_env`]) runs after deserialization so
    /// `BROADCAST_CONNECTION` / `PUSHER_*` / `REDIS_URL` win over the file.
    ///
    /// # Errors
    ///
    /// [`BroadcastError::Serialization`] when the `[broadcasting]` table exists
    /// but does not deserialize.
    pub fn from_loader(loader: &ConfigLoader) -> Result<Self> {
        let table = match loader.get_key::<BroadcastingTable>("broadcasting") {
            Ok(table) => table,
            Err(error) => {
                if loader.inner().get_table("broadcasting").is_err() {
                    BroadcastingTable::default()
                } else {
                    return Err(BroadcastError::Serialization(error.to_string()));
                }
            }
        };
        let mut config = Self {
            default: table
                .default
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_CONNECTION.to_string()),
            #[cfg(feature = "pusher")]
            pusher: table.connections.pusher,
            #[cfg(feature = "redis")]
            redis: table.connections.redis,
        };
        config.apply_env();
        Ok(config)
    }

    /// Apply the documented environment overrides.
    ///
    /// `BROADCAST_CONNECTION` replaces the default selector; the `PUSHER_*`
    /// family and `BROADCAST_REDIS_URL`/`REDIS_URL` build or complete the
    /// matching driver config even when no file section exists. A blank value is
    /// ignored. The loader's `__` separator means single-underscore variables
    /// never reach the nested table, so this bridge is the only `.env` path.
    pub fn apply_env(&mut self) {
        if let Some(connection) = env_non_empty("BROADCAST_CONNECTION") {
            self.default = connection;
        }
        #[cfg(feature = "pusher")]
        self.apply_pusher_env();
        #[cfg(feature = "redis")]
        self.apply_redis_env();
    }

    /// Build/complete the Pusher config from `PUSHER_*` variables.
    #[cfg(feature = "pusher")]
    fn apply_pusher_env(&mut self) {
        let app_id = env_non_empty("PUSHER_APP_ID");
        let key = env_non_empty("PUSHER_APP_KEY");
        let secret = env_non_empty("PUSHER_APP_SECRET");
        let cluster = env_non_empty("PUSHER_APP_CLUSTER");
        let host = env_non_empty("PUSHER_HOST");
        let port = env_non_empty("PUSHER_PORT").and_then(|value| value.parse::<u16>().ok());
        let scheme = env_non_empty("PUSHER_SCHEME");
        // Any Pusher env var present means the driver is being configured;
        // build a config from the file values (if any) then overlay the env.
        let any = app_id.is_some()
            || key.is_some()
            || secret.is_some()
            || cluster.is_some()
            || host.is_some()
            || port.is_some()
            || scheme.is_some();
        if !any {
            return;
        }
        let mut config = self.pusher.take().unwrap_or_default();
        if let Some(value) = app_id {
            config.app_id = value;
        }
        if let Some(value) = key {
            config.key = value;
        }
        if let Some(value) = secret {
            config.secret = value;
        }
        if let Some(value) = cluster {
            config.cluster = Some(value);
        }
        if let Some(value) = host {
            config.host = Some(value);
        }
        if let Some(value) = port {
            config.port = Some(value);
        }
        if let Some(value) = scheme {
            config.scheme = Some(value);
        }
        self.pusher = Some(config);
    }

    /// Build/complete the Redis config from `BROADCAST_REDIS_URL` / `REDIS_URL`.
    #[cfg(feature = "redis")]
    fn apply_redis_env(&mut self) {
        let url = env_non_empty("BROADCAST_REDIS_URL").or_else(|| env_non_empty("REDIS_URL"));
        let prefix = env_non_empty("BROADCAST_REDIS_PREFIX");
        if url.is_none() && prefix.is_none() {
            return;
        }
        let mut config = self.redis.take().unwrap_or_default();
        if let Some(value) = url {
            config.url = value;
        }
        if let Some(value) = prefix {
            config.prefix = value;
        }
        self.redis = Some(config);
    }

    /// The connection names that are actually configured (for introspection).
    pub fn configured_connections(&self) -> Vec<&'static str> {
        // `mut` is only needed when a driver feature pushes onto the list.
        #[allow(unused_mut)]
        let mut names = vec![super::CONNECTION_HUB];
        #[cfg(feature = "pusher")]
        if self.pusher.is_some() {
            names.push(super::CONNECTION_PUSHER);
        }
        #[cfg(feature = "redis")]
        if self.redis.is_some() {
            names.push(super::CONNECTION_REDIS);
        }
        names
    }
}

/// Deserialized shape of the `[broadcasting]` table.
#[derive(Debug, Default, Deserialize)]
struct BroadcastingTable {
    /// `broadcasting.default` connection selector.
    #[serde(default)]
    default: Option<String>,
    /// `[broadcasting.connections.*]` driver tables.
    ///
    /// Only read when a driver feature is compiled in; otherwise the map is
    /// parsed and discarded (a config that declares connections without the
    /// matching feature simply has no effect).
    #[cfg_attr(not(any(feature = "pusher", feature = "redis")), allow(dead_code))]
    #[serde(default)]
    connections: ConnectionsTable,
}

/// Deserialized `[broadcasting.connections.*]` map.
#[derive(Debug, Default, Deserialize)]
struct ConnectionsTable {
    /// `[broadcasting.connections.pusher]`.
    #[cfg(feature = "pusher")]
    #[serde(default)]
    pusher: Option<PusherConfig>,
    /// `[broadcasting.connections.redis]`.
    #[cfg(feature = "redis")]
    #[serde(default)]
    redis: Option<RedisConfig>,
}
