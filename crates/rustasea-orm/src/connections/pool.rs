//! Pool tuning and endpoint-overlay config types.
//!
//! Split out of [`crate::connections`] so the parent module stays focused on
//! the top-level `[database]` table and connection resolution. Both types are
//! re-exported from the parent, so existing import paths keep resolving.

use std::time::Duration;

use serde::Deserialize;

use crate::db::PoolSettings;

/// Pool tuning overrides read from `[database.pool]` / `[database.connections.*.pool]`.
///
/// Every field is optional; unset values fall back to [`PoolSettings::default`].
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PoolConfig {
    /// Minimum number of idle connections kept warm.
    #[serde(default)]
    pub min: Option<u32>,
    /// Maximum connections the pool will open.
    #[serde(default)]
    pub max: Option<u32>,
    /// Seconds before an idle connection is reaped.
    #[serde(default)]
    pub idle_timeout: Option<u64>,
}

impl PoolConfig {
    /// Convert the config into runtime [`PoolSettings`], keeping ORM defaults
    /// for unset fields.
    pub(crate) fn to_settings(&self) -> PoolSettings {
        let mut settings = PoolSettings::default();
        if let Some(min) = self.min {
            settings.min_connections = min;
        }
        if let Some(max) = self.max {
            settings.max_connections = max;
        }
        if let Some(idle) = self.idle_timeout {
            settings.idle_timeout = Some(Duration::from_secs(idle));
        }
        settings
    }
}

/// One optional read or write endpoint overlay (`[….read]` / `[….write]`).
///
/// Every field is optional. When an endpoint omits a field it falls back to the
/// primary connection's value, so a replica that shares credentials with the
/// primary only needs `host` (and maybe `port`). Supplying `url` replaces the
/// whole endpoint (still validated against the primary `driver`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EndpointConfig {
    /// Full endpoint URL; when set it takes precedence over the granular fields.
    #[serde(default)]
    pub url: Option<String>,
    /// Host name or IP address (network drivers).
    #[serde(default)]
    pub host: Option<String>,
    /// TCP port (network drivers); falls back to the primary when omitted.
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
}
