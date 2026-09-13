//! Connection configuration for the MongoDB document store.
//!
//! [`MongoConfig`] is loaded from environment variables ([`MongoConfig::from_env`])
//! or from a `config/mongo.toml` document ([`MongoConfig::from_toml`] /
//! [`MongoConfig::from_toml_file`]). Validation is deliberately shallow: an
//! empty URI or database is rejected eagerly, while scheme errors surface from
//! the driver as [`crate::MongoError::Configuration`].

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use crate::error::{MongoError, Result};

/// Environment variable holding the connection string.
pub const ENV_URI: &str = "MONGODB_URI";

/// Optional environment variable overriding the database name.
pub const ENV_DATABASE: &str = "MONGODB_DATABASE";

/// Database used when neither the environment nor the config file names one.
pub const DEFAULT_DATABASE: &str = "rustasea";

/// Connection pool tuning for the MongoDB driver.
///
/// All fields are optional; omitted values fall back to the driver defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct MongoPoolConfig {
    /// Minimum number of idle connections kept warm.
    #[serde(default)]
    pub min: Option<u32>,
    /// Maximum number of connections the pool may open.
    #[serde(default)]
    pub max: Option<u32>,
    /// Seconds before an idle connection is reaped.
    #[serde(default)]
    pub idle_timeout: Option<u64>,
    /// Application name reported to the server (shows in `currentOp`).
    #[serde(default)]
    pub app_name: Option<String>,
}

impl MongoPoolConfig {
    /// Convert to a driver [`mongodb::options::ClientOptions`] fragment.
    ///
    /// Only the fields that were set are applied, leaving every other driver
    /// default untouched.
    pub fn apply(&self, options: &mut mongodb::options::ClientOptions) {
        if let Some(min) = self.min {
            options.min_pool_size = Some(min);
        }
        if let Some(max) = self.max {
            options.max_pool_size = Some(max);
        }
        if let Some(secs) = self.idle_timeout {
            options.max_idle_time = Some(Duration::from_secs(secs));
        }
        if let Some(app) = &self.app_name {
            options.app_name = Some(app.clone());
        }
    }
}

/// MongoDB connection settings.
///
/// Build with [`MongoConfig::new`], [`MongoConfig::from_env`], or one of the
/// `toml` loaders. [`MongoConfig::validate`] is called by the loaders and by
/// [`crate::MongoClient::connect`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MongoConfig {
    /// Connection string (`mongodb://` or `mongodb+srv://`).
    pub uri: String,
    /// Default database for collection access.
    pub database: String,
    /// Optional pool tuning.
    #[serde(default)]
    pub pool: Option<MongoPoolConfig>,
}

impl MongoConfig {
    /// Build a config from an explicit URI and database.
    pub fn new(uri: impl Into<String>, database: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            database: database.into(),
            pool: None,
        }
    }

    /// Attach pool tuning (builder style).
    pub fn with_pool(mut self, pool: MongoPoolConfig) -> Self {
        self.pool = Some(pool);
        self
    }

    /// Build from the resolved parts of a named-connection entry.
    ///
    /// This is the interop seam for `rustasea-orm`'s named-connections resolver:
    /// the resolver owns `[database.connections.<name>]` parsing and passes the
    /// connection's URI and database here, so this crate never depends on the
    /// ORM (avoiding a dependency cycle).
    ///
    /// **Environment precedence** mirrors [`MongoConfig::from_env`]: `MONGODB_URI`
    /// overrides `uri` and `MONGODB_DATABASE` overrides `database`. The database
    /// falls back to [`DEFAULT_DATABASE`] when neither the argument nor the
    /// environment names one.
    ///
    /// # Errors
    ///
    /// [`MongoError::Configuration`] when no URI is available (neither the env
    /// var nor `uri` is set) or the resulting config fails
    /// [`MongoConfig::validate`].
    pub fn from_parts(uri: Option<&str>, database: Option<&str>) -> Result<Self> {
        let uri = std::env::var(ENV_URI)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                uri.map(str::to_string)
                    .filter(|value| !value.trim().is_empty())
            })
            .ok_or_else(|| {
                MongoError::Configuration(format!(
                    "{ENV_URI} is not set and the connection declares no uri"
                ))
            })?;
        let database = std::env::var(ENV_DATABASE)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                database
                    .map(str::to_string)
                    .filter(|value| !value.trim().is_empty())
            })
            .unwrap_or_else(|| DEFAULT_DATABASE.to_string());
        let config = Self {
            uri,
            database,
            pool: None,
        };
        config.validate()?;
        Ok(config)
    }

    /// Load from `MONGODB_URI` (+ optional `MONGODB_DATABASE`).
    ///
    /// Returns [`MongoError::Configuration`] when `MONGODB_URI` is unset or
    /// empty. The database defaults to [`DEFAULT_DATABASE`] when
    /// `MONGODB_DATABASE` is unset.
    pub fn from_env() -> Result<Self> {
        let uri = std::env::var(ENV_URI)
            .map_err(|_| MongoError::Configuration(format!("{ENV_URI} is not set")))?;
        let database = std::env::var(ENV_DATABASE).unwrap_or_else(|_| DEFAULT_DATABASE.to_string());
        let config = Self {
            uri,
            database,
            pool: None,
        };
        config.validate()?;
        Ok(config)
    }

    /// Parse a `config/mongo.toml` document.
    ///
    /// The document wraps the settings in a `[mongo]` table:
    ///
    /// ```toml
    /// [mongo]
    /// uri = "mongodb://localhost:27017"
    /// database = "rustasea"
    /// ```
    pub fn from_toml(input: &str) -> Result<Self> {
        let document: MongoDocument = toml::from_str(input)
            .map_err(|e| MongoError::Configuration(format!("invalid mongo.toml: {e}")))?;
        document.mongo.validate()?;
        Ok(document.mongo)
    }

    /// Parse a `config/mongo.toml` file from disk.
    pub fn from_toml_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path)
            .map_err(|e| MongoError::Configuration(format!("{}: {e}", path.display())))?;
        Self::from_toml(&contents)
    }

    /// Reject empty URI/database early with a typed error.
    pub fn validate(&self) -> Result<()> {
        if self.uri.trim().is_empty() {
            return Err(MongoError::Configuration("uri must not be empty".into()));
        }
        if !self.uri.starts_with("mongodb://") && !self.uri.starts_with("mongodb+srv://") {
            return Err(MongoError::Configuration(format!(
                "uri must start with mongodb:// or mongodb+srv:// (got {})",
                self.uri
            )));
        }
        if self.database.trim().is_empty() {
            return Err(MongoError::Configuration(
                "database must not be empty".into(),
            ));
        }
        Ok(())
    }
}

/// Internal wrapper matching the document's top-level `[mongo]` table.
#[derive(Debug, Deserialize)]
struct MongoDocument {
    /// The single `[mongo]` table.
    mongo: MongoConfig,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes tests that mutate process-global environment variables.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Restore an environment variable to its previous value (or unset it).
    fn restore(key: &str, value: Option<String>) {
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }

    /// Arguments supply the URI and database when the environment is unset.
    #[test]
    fn from_parts_uses_arguments_without_env() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let uri = std::env::var(ENV_URI).ok();
        let database = std::env::var(ENV_DATABASE).ok();
        std::env::remove_var(ENV_URI);
        std::env::remove_var(ENV_DATABASE);

        let config = MongoConfig::from_parts(Some("mongodb://127.0.0.1:27017"), Some("app"))
            .expect("build from parts");
        assert_eq!(config.uri, "mongodb://127.0.0.1:27017");
        assert_eq!(config.database, "app");

        restore(ENV_URI, uri);
        restore(ENV_DATABASE, database);
    }

    /// An omitted database falls back to [`DEFAULT_DATABASE`].
    #[test]
    fn from_parts_defaults_database() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let uri = std::env::var(ENV_URI).ok();
        let database = std::env::var(ENV_DATABASE).ok();
        std::env::remove_var(ENV_URI);
        std::env::remove_var(ENV_DATABASE);

        let config = MongoConfig::from_parts(Some("mongodb://127.0.0.1:27017"), None)
            .expect("build from parts");
        assert_eq!(config.database, DEFAULT_DATABASE);

        restore(ENV_URI, uri);
        restore(ENV_DATABASE, database);
    }

    /// Environment variables override the supplied arguments.
    #[test]
    fn from_parts_env_overrides_arguments() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let uri = std::env::var(ENV_URI).ok();
        let database = std::env::var(ENV_DATABASE).ok();
        std::env::set_var(ENV_URI, "mongodb://env-host:27017");
        std::env::set_var(ENV_DATABASE, "env-db");

        let config = MongoConfig::from_parts(Some("mongodb://arg-host:27017"), Some("arg-db"))
            .expect("build from parts");
        assert_eq!(config.uri, "mongodb://env-host:27017");
        assert_eq!(config.database, "env-db");

        restore(ENV_URI, uri);
        restore(ENV_DATABASE, database);
    }

    /// Neither an argument nor the environment URI is a typed error.
    #[test]
    fn from_parts_requires_a_uri() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let uri = std::env::var(ENV_URI).ok();
        let database = std::env::var(ENV_DATABASE).ok();
        std::env::remove_var(ENV_URI);
        std::env::remove_var(ENV_DATABASE);

        let error = MongoConfig::from_parts(None, Some("app")).unwrap_err();
        assert!(error.is_configuration(), "got {error:?}");

        restore(ENV_URI, uri);
        restore(ENV_DATABASE, database);
    }
}
