//! Typed `[queue]` configuration — Laravel 13.x `config/queue.php` parity.
//!
//! [`QueueConfig`] mirrors the shape of Laravel's `config/queue.php`:
//! a `default` connection selector, a `connections` map of named connections,
//! a `batching` block for `Bus::batch()`, and a `failed` block for the
//! dead-letter store.
//!
//! ```toml
//! [queue]
//! default = "database"
//! [queue.connections.sync]
//! driver = "sync"
//! [queue.connections.database]
//! driver = "database"
//! table = "jobs"
//! [queue.batching]
//! table = "job_batches"
//! [queue.failed]
//! driver = "database-uuids"
//! table = "failed_jobs"
//! ```
//!
//! The table is deserialized from [`rustasea_config::ConfigLoader`] via
//! [`QueueConfig::from_loader`]. A missing `[queue]` table is tolerated (an
//! empty config with the [`DEFAULT_CONNECTION`] selector), while a malformed or
//! invalid one surfaces a typed [`QueueConfigError`].
//!
//! ## Implemented vs. recognised drivers
//!
//! Only `sync`, `database` and `redis` are implemented. The remaining Laravel
//! drivers (`beanstalkd`, `sqs`, `deferred`, `background`, `failover`) are
//! recognised by name but **not yet implemented**; selecting one is a typed
//! [`QueueConfigError::UnsupportedDriver`] at load time rather than a silent
//! fallback, so a migrated production config can never quietly degrade to the
//! inline `sync` driver.
//!
//! ## Migration linkage
//!
//! [`QueueConfig::jobs_table`] and [`QueueConfig::failed_table`] return the
//! configured `jobs` / `failed_jobs` table names. They feed the database driver
//! and the parameterized migrations in [`crate::migrations`]
//! (`migrator_with_tables`); see that module for how the DDL links to these
//! keys. `batching.table` is exposed via [`QueueConfig::batching`] but has no
//! migration yet (documented in [`crate::migrations`]).

use std::collections::BTreeMap;

use serde::Deserialize;

use rustasea_config::ConfigLoader;

use crate::error::QueueConfigError;

/// Result alias for queue configuration parsing / validation.
pub type ConfigResult<T> = std::result::Result<T, QueueConfigError>;

/// Connection used when `queue.default` is absent.
pub const DEFAULT_CONNECTION: &str = "database";

/// Driver name for the inline `sync` connection.
pub const DRIVER_SYNC: &str = "sync";
/// Driver name for the `database` connection.
pub const DRIVER_DATABASE: &str = "database";
/// Driver name for the `redis` connection (requires the `redis` feature).
pub const DRIVER_REDIS: &str = "redis";

/// Default queue name when a connection omits `queue`.
pub const DEFAULT_QUEUE: &str = "default";
/// Default reservation timeout in seconds (Laravel parity).
pub const DEFAULT_RETRY_AFTER: u64 = 90;
/// Default `jobs` table name.
pub const DEFAULT_JOBS_TABLE: &str = "jobs";
/// Default `job_batches` table name.
pub const DEFAULT_BATCHES_TABLE: &str = "job_batches";
/// Default `failed_jobs` table name.
pub const DEFAULT_FAILED_TABLE: &str = "failed_jobs";
/// Default `failed.driver` (Laravel parity).
pub const DEFAULT_FAILED_DRIVER: &str = "database-uuids";

/// Serde default for [`QueueConfig::default`].
fn default_connection_name() -> String {
    DEFAULT_CONNECTION.to_string()
}

/// Serde default for [`ConnectionConfig::queue`].
fn default_queue() -> String {
    DEFAULT_QUEUE.to_string()
}

/// Serde default for [`ConnectionConfig::retry_after`].
fn default_retry_after() -> u64 {
    DEFAULT_RETRY_AFTER
}

/// Serde default for [`BatchingConfig::table`].
fn default_batches_table() -> String {
    DEFAULT_BATCHES_TABLE.to_string()
}

/// Serde default for [`FailedConfig::driver`].
fn default_failed_driver() -> String {
    DEFAULT_FAILED_DRIVER.to_string()
}

/// Serde default for [`FailedConfig::table`].
fn default_failed_table() -> String {
    DEFAULT_FAILED_TABLE.to_string()
}

/// Read an environment variable, treating unset or blank values as absent.
fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// One `[queue.connections.<name>]` entry.
///
/// The struct is the union of the fields the implemented drivers read; a
/// connection simply leaves the fields its driver does not use unset. `driver`
/// defaults to the empty string so a malformed entry is caught by validation
/// rather than silently accepted.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ConnectionConfig {
    /// Driver selector (`sync`, `database`, `redis`, …).
    #[serde(default)]
    pub driver: String,
    /// Named `[database.connections.<name>]` backing a `database`/`redis` driver.
    #[serde(default)]
    pub connection: Option<String>,
    /// Table name (the `database` driver's jobs table).
    #[serde(default)]
    pub table: Option<String>,
    /// Default queue this connection drains when a dispatch names none.
    #[serde(default = "default_queue")]
    pub queue: String,
    /// Reservation timeout in seconds (Laravel `retry_after`).
    #[serde(default = "default_retry_after")]
    pub retry_after: u64,
    /// Defer the push until the enclosing transaction commits (parsed for parity).
    #[serde(default)]
    pub after_commit: bool,
    /// Seconds `BRPOP` blocks on an empty queue (`redis`; `None` = short poll).
    #[serde(default)]
    pub block_for: Option<u64>,
}

impl Default for ConnectionConfig {
    /// An entry with no driver and Laravel's default queue/retry settings.
    fn default() -> Self {
        Self {
            driver: String::new(),
            connection: None,
            table: None,
            queue: default_queue(),
            retry_after: default_retry_after(),
            after_commit: false,
            block_for: None,
        }
    }
}

impl ConnectionConfig {
    /// Whether this connection declares the inline `sync` driver.
    pub fn is_sync(&self) -> bool {
        self.driver.eq_ignore_ascii_case(DRIVER_SYNC)
    }

    /// Whether this connection declares the `database` driver.
    pub fn is_database(&self) -> bool {
        self.driver.eq_ignore_ascii_case(DRIVER_DATABASE)
    }

    /// Whether this connection declares the `redis` driver.
    pub fn is_redis(&self) -> bool {
        self.driver.eq_ignore_ascii_case(DRIVER_REDIS)
    }
}

/// Typed `[queue.batching]` table.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BatchingConfig {
    /// Named DB connection backing the batches table (`None` = ORM default).
    #[serde(default)]
    pub database: Option<String>,
    /// Batches table name (Laravel default `job_batches`).
    #[serde(default = "default_batches_table")]
    pub table: String,
}

impl Default for BatchingConfig {
    /// Laravel defaults: ORM default connection, `job_batches` table.
    fn default() -> Self {
        Self {
            database: None,
            table: default_batches_table(),
        }
    }
}

/// Typed `[queue.failed]` table (the dead-letter store).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FailedConfig {
    /// Failed-job driver; `database-uuids` is the implemented driver.
    #[serde(default = "default_failed_driver")]
    pub driver: String,
    /// Named DB connection backing the failed table (`None` = ORM default).
    #[serde(default)]
    pub database: Option<String>,
    /// Failed-jobs table name (Laravel default `failed_jobs`).
    #[serde(default = "default_failed_table")]
    pub table: String,
}

impl Default for FailedConfig {
    /// Laravel defaults: `database-uuids` driver, `failed_jobs` table.
    fn default() -> Self {
        Self {
            driver: default_failed_driver(),
            database: None,
            table: default_failed_table(),
        }
    }
}

/// Typed `[queue]` table (Laravel 13.x `config/queue.php` shape).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct QueueConfig {
    /// Connection used when a dispatch names none.
    #[serde(default = "default_connection_name")]
    pub default: String,
    /// Named connections, ordered for deterministic iteration.
    #[serde(default)]
    pub connections: BTreeMap<String, ConnectionConfig>,
    /// `[queue.batching]` settings.
    #[serde(default)]
    pub batching: BatchingConfig,
    /// `[queue.failed]` settings.
    #[serde(default)]
    pub failed: FailedConfig,
}

impl Default for QueueConfig {
    /// An empty config selecting the [`DEFAULT_CONNECTION`] with no connections.
    fn default() -> Self {
        Self {
            default: default_connection_name(),
            connections: BTreeMap::new(),
            batching: BatchingConfig::default(),
            failed: FailedConfig::default(),
        }
    }
}

impl QueueConfig {
    /// Deserialize `[queue]` from a layered [`ConfigLoader`] and validate it.
    ///
    /// A missing `[queue]` table yields [`QueueConfig::default`] (tolerated,
    /// matching the loader's missing-file policy); a table that exists but does
    /// not deserialize surfaces [`QueueConfigError::Invalid`], and a table that
    /// deserializes but names an unknown connection / unsupported driver /
    /// missing required field surfaces the matching typed error. The documented
    /// `QUEUE_CONNECTION` override is applied by [`QueueConfig::apply_env`]
    /// before validation.
    ///
    /// # Errors
    ///
    /// [`QueueConfigError::Invalid`] on malformed TOML, or any
    /// [`QueueConfigError`] variant from [`QueueConfig::validate`].
    pub fn from_loader(loader: &ConfigLoader) -> ConfigResult<Self> {
        let mut config = match loader.get_key::<QueueConfig>("queue") {
            Ok(config) => config,
            Err(error) => {
                if loader.inner().get_table("queue").is_err() {
                    return Ok(QueueConfig::default());
                }
                return Err(QueueConfigError::Invalid(error.to_string()));
            }
        };
        config.apply_env();
        config.validate()?;
        Ok(config)
    }

    /// Apply the documented single-underscore `QUEUE_*` environment overrides.
    ///
    /// `QUEUE_CONNECTION` replaces the default connection selector
    /// (`queue.default`); the environment wins over the file and a blank value
    /// is ignored. The loader's `__` separator means single-underscore variables
    /// never reach the nested `[queue]` table, so this bridge is the only path
    /// for `.env` parity.
    pub fn apply_env(&mut self) {
        if let Some(connection) = env_non_empty("QUEUE_CONNECTION") {
            self.default = connection;
        }
    }

    /// Validate the connection selectors and driver fields.
    ///
    /// An empty `connections` map is accepted (no queue table configured); when
    /// connections are declared, `default` must name one, every connection must
    /// declare an implemented driver, and the `database` driver must declare a
    /// `table`. The `failed.driver` must be `database-uuids` (or `database`).
    ///
    /// # Errors
    ///
    /// [`QueueConfigError::UnknownConnection`] when `default` is undeclared,
    /// [`QueueConfigError::UnsupportedDriver`] for an unimplemented driver, or
    /// [`QueueConfigError::MissingField`] for a missing `table`.
    pub fn validate(&self) -> ConfigResult<()> {
        if self.connections.is_empty() {
            return Ok(());
        }
        if !self.connections.contains_key(&self.default) {
            return Err(QueueConfigError::UnknownConnection {
                name: self.default.clone(),
            });
        }
        for (name, connection) in &self.connections {
            let driver = connection.driver.trim();
            match driver.to_ascii_lowercase().as_str() {
                DRIVER_SYNC | DRIVER_DATABASE | DRIVER_REDIS => {}
                _ => {
                    return Err(QueueConfigError::UnsupportedDriver {
                        connection: name.clone(),
                        driver: connection.driver.clone(),
                    });
                }
            }
            if driver.eq_ignore_ascii_case(DRIVER_DATABASE)
                && connection.table.as_deref().unwrap_or("").trim().is_empty()
            {
                return Err(QueueConfigError::MissingField {
                    connection: name.clone(),
                    field: "table".to_string(),
                });
            }
        }
        let failed = self.failed.driver.trim();
        if !failed.eq_ignore_ascii_case(DEFAULT_FAILED_DRIVER)
            && !failed.eq_ignore_ascii_case(DRIVER_DATABASE)
        {
            return Err(QueueConfigError::UnsupportedDriver {
                connection: "failed".to_string(),
                driver: self.failed.driver.clone(),
            });
        }
        Ok(())
    }

    /// Look up a connection declaration by name.
    ///
    /// # Errors
    ///
    /// [`QueueConfigError::UnknownConnection`] when no such connection exists.
    pub fn connection(&self, name: &str) -> ConfigResult<&ConnectionConfig> {
        self.connections
            .get(name)
            .ok_or_else(|| QueueConfigError::UnknownConnection {
                name: name.to_string(),
            })
    }

    /// Look up the default connection declaration.
    ///
    /// # Errors
    ///
    /// [`QueueConfigError::UnknownConnection`] when `default` is undeclared.
    pub fn default_connection(&self) -> ConfigResult<&ConnectionConfig> {
        self.connection(&self.default)
    }

    /// The `jobs` table for `connection` (its `table`, else [`DEFAULT_JOBS_TABLE`]).
    pub fn jobs_table(&self, connection: &ConnectionConfig) -> String {
        connection
            .table
            .clone()
            .filter(|table| !table.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_JOBS_TABLE.to_string())
    }

    /// The configured `failed_jobs` table name.
    pub fn failed_table(&self) -> &str {
        &self.failed.table
    }

    /// The configured `job_batches` table name.
    pub fn batches_table(&self) -> &str {
        &self.batching.table
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `QueueConfig` directly (no loader) for validation tests.
    fn config_with(name: &str, driver: &str, table: Option<&str>) -> QueueConfig {
        let mut connections = BTreeMap::new();
        connections.insert(
            name.to_string(),
            ConnectionConfig {
                driver: driver.to_string(),
                table: table.map(str::to_string),
                ..ConnectionConfig::default()
            },
        );
        QueueConfig {
            default: name.to_string(),
            connections,
            ..QueueConfig::default()
        }
    }

    /// A `database` connection without a table is a typed missing-field error.
    #[test]
    fn database_without_table_is_missing_field() {
        let config = config_with("database", DRIVER_DATABASE, None);
        assert_eq!(
            config.validate(),
            Err(QueueConfigError::MissingField {
                connection: "database".to_string(),
                field: "table".to_string(),
            })
        );
    }

    /// An unrecognised driver names the offending connection and driver.
    #[test]
    fn unknown_driver_is_typed_error() {
        let config = config_with("sqs", "sqs", None);
        assert_eq!(
            config.validate(),
            Err(QueueConfigError::UnsupportedDriver {
                connection: "sqs".to_string(),
                driver: "sqs".to_string(),
            })
        );
    }

    /// `default` naming an undeclared connection is a typed error.
    #[test]
    fn unknown_default_is_typed_error() {
        let config = config_with("database", DRIVER_DATABASE, Some(DEFAULT_JOBS_TABLE));
        let mut broken = config.clone();
        broken.default = "missing".to_string();
        assert_eq!(
            broken.validate(),
            Err(QueueConfigError::UnknownConnection {
                name: "missing".to_string(),
            })
        );
        assert!(config.validate().is_ok());
    }

    /// An empty connection map is tolerated (no `[queue]` table configured).
    #[test]
    fn empty_connections_skip_validation() {
        assert!(QueueConfig::default().validate().is_ok());
    }

    /// The accessors expose the configured table names with defaults.
    #[test]
    fn table_accessors_expose_config_values() {
        let mut config = config_with("database", DRIVER_DATABASE, Some("my_jobs"));
        config.failed.table = "my_failed".to_string();
        config.batching.table = "my_batches".to_string();
        let connection = config.connection("database").expect("declared");
        assert_eq!(config.jobs_table(connection), "my_jobs");
        assert_eq!(config.failed_table(), "my_failed");
        assert_eq!(config.batches_table(), "my_batches");
    }
}
