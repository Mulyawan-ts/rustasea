//! Real database connection foundation backed by `sqlx`.
//!
//! [`DbPool`] wraps one live `sqlx` pool per supported driver; [`DbPool::fetch_json`]
//! reads, [`DbPool::execute_bind`] writes, and [`DbPool::query_raw`]/[`DbPool::execute_raw`] run raw SQL.

use crate::connections::{ConnectionConfig, EndpointConfig};
use crate::error::{ConnectionError, OrmError, Result};
use std::time::Duration;

mod adapt;
mod exec;
mod raw;

/// Maximum connections held by a pool (SQLite in-memory is capped at one).
const DEFAULT_MAX_CONNECTIONS: u32 = 10;

/// Time to wait for a connection before `connect`/`acquire` fails.
const DEFAULT_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(30);

/// Tunable pool limits applied when opening a connection. [`DbPool::connect`]
/// uses [`PoolSettings::default`]; config callers pass `[database.pool]` values.
#[derive(Debug, Clone, Copy)]
pub struct PoolSettings {
    /// Minimum number of idle connections kept warm.
    pub min_connections: u32,
    /// Maximum connections the pool will open.
    pub max_connections: u32,
    /// Time to wait for a connection before `connect`/`acquire` fails.
    pub acquire_timeout: Duration,
    /// Idle lifetime after which a pooled connection is reaped.
    pub idle_timeout: Option<Duration>,
}
impl Default for PoolSettings {
    /// ORM defaults: `min = 0`, `max = 10`, 30s acquire timeout, no idle reaping.
    fn default() -> Self {
        Self {
            min_connections: 0,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            acquire_timeout: DEFAULT_ACQUIRE_TIMEOUT,
            idle_timeout: None,
        }
    }
}

/// A live connection pool for one supported database driver.
#[derive(Debug, Clone)]
pub enum DbPool {
    /// SQLite pool (file-backed or `:memory:`).
    #[cfg(feature = "sqlite")]
    Sqlite(sqlx::SqlitePool),
    /// PostgreSQL pool.
    #[cfg(feature = "postgres")]
    Postgres(sqlx::PgPool),
    /// MySQL pool.
    #[cfg(feature = "mysql")]
    MySql(sqlx::MySqlPool),
}

impl DbPool {
    /// Connect to `url`, selecting the driver from its scheme. Unknown schemes
    /// and driver failures surface as [`OrmError::Pool`]; a recognised scheme
    /// whose driver feature is not compiled in surfaces as
    /// [`OrmError::UnsupportedDriver`].
    pub async fn connect(url: &str) -> Result<Self> {
        Self::connect_with_settings(url, PoolSettings::default()).await
    }

    /// Connect to `url` with explicit [`PoolSettings`]. SQLite in-memory URLs
    /// pin to one connection, since each connection sees a distinct database.
    pub async fn connect_with_settings(url: &str, settings: PoolSettings) -> Result<Self> {
        let scheme = url.split(':').next().unwrap_or_default();
        match scheme {
            "sqlite" => Self::connect_sqlite(url, settings).await,
            "postgres" | "postgresql" => Self::connect_postgres(url, settings).await,
            "mysql" => Self::connect_mysql(url, settings).await,
            other => Err(OrmError::Pool(format!(
                "invalid database URL `{url}`: unknown scheme `{other}`"
            ))),
        }
    }

    /// The dialect name (`sqlite` / `postgres` / `mysql`).
    pub fn dialect(&self) -> &'static str {
        match self {
            #[cfg(feature = "sqlite")]
            DbPool::Sqlite(_) => "sqlite",
            #[cfg(feature = "postgres")]
            DbPool::Postgres(_) => "postgres",
            #[cfg(feature = "mysql")]
            DbPool::MySql(_) => "mysql",
            #[cfg(not(any(feature = "sqlite", feature = "postgres", feature = "mysql")))]
            _ => "unknown",
        }
    }

    /// Execute `SELECT 1` to verify the pool can reach the database.
    pub async fn ping(&self) -> Result<()> {
        match self {
            #[cfg(feature = "sqlite")]
            DbPool::Sqlite(pool) => {
                sqlx::query("SELECT 1").execute(pool).await?;
            }
            #[cfg(feature = "postgres")]
            DbPool::Postgres(pool) => {
                sqlx::query("SELECT 1").execute(pool).await?;
            }
            #[cfg(feature = "mysql")]
            DbPool::MySql(pool) => {
                sqlx::query("SELECT 1").execute(pool).await?;
            }
            #[cfg(not(any(feature = "sqlite", feature = "postgres", feature = "mysql")))]
            _ => {}
        }
        Ok(())
    }

    /// Gracefully close every connection in the pool.
    pub async fn close(&self) {
        match self {
            #[cfg(feature = "sqlite")]
            DbPool::Sqlite(pool) => pool.close().await,
            #[cfg(feature = "postgres")]
            DbPool::Postgres(pool) => pool.close().await,
            #[cfg(feature = "mysql")]
            DbPool::MySql(pool) => pool.close().await,
            #[cfg(not(any(feature = "sqlite", feature = "postgres", feature = "mysql")))]
            _ => {}
        }
    }

    /// The inner SQLite pool, when this handle is SQLite.
    #[cfg(feature = "sqlite")]
    pub fn sqlite(&self) -> Option<&sqlx::SqlitePool> {
        match self {
            DbPool::Sqlite(pool) => Some(pool),
            #[cfg(feature = "postgres")]
            DbPool::Postgres(_) => None,
            #[cfg(feature = "mysql")]
            DbPool::MySql(_) => None,
        }
    }

    /// The inner Postgres pool, when this handle is Postgres.
    #[cfg(feature = "postgres")]
    pub fn postgres(&self) -> Option<&sqlx::PgPool> {
        match self {
            DbPool::Postgres(pool) => Some(pool),
            #[cfg(feature = "sqlite")]
            DbPool::Sqlite(_) => None,
            #[cfg(feature = "mysql")]
            DbPool::MySql(_) => None,
        }
    }

    /// The inner MySQL pool, when this handle is MySQL.
    #[cfg(feature = "mysql")]
    pub fn mysql(&self) -> Option<&sqlx::MySqlPool> {
        match self {
            DbPool::MySql(pool) => Some(pool),
            #[cfg(feature = "sqlite")]
            DbPool::Sqlite(_) => None,
            #[cfg(feature = "postgres")]
            DbPool::Postgres(_) => None,
        }
    }

    /// A process-unique identity for this pool handle: clones share one
    /// connection-options allocation; separately opened pools differ.
    pub fn identity(&self) -> usize {
        use std::sync::Arc;
        match self {
            #[cfg(feature = "sqlite")]
            DbPool::Sqlite(pool) => Arc::as_ptr(&pool.connect_options()) as *const () as usize,
            #[cfg(feature = "postgres")]
            DbPool::Postgres(pool) => Arc::as_ptr(&pool.connect_options()) as *const () as usize,
            #[cfg(feature = "mysql")]
            DbPool::MySql(pool) => Arc::as_ptr(&pool.connect_options()) as *const () as usize,
            #[cfg(not(any(feature = "sqlite", feature = "postgres", feature = "mysql")))]
            _ => 0,
        }
    }

    /// Connect a SQLite pool; in-memory URLs are pinned to one connection.
    #[cfg(feature = "sqlite")]
    async fn connect_sqlite(url: &str, settings: PoolSettings) -> Result<Self> {
        use std::str::FromStr;

        let options = sqlx::sqlite::SqliteConnectOptions::from_str(url)
            .map_err(|error| OrmError::Pool(error.to_string()))?
            .create_if_missing(true);

        let mut pool_options = sqlx::sqlite::SqlitePoolOptions::new()
            .min_connections(settings.min_connections)
            .max_connections(settings.max_connections)
            .acquire_timeout(settings.acquire_timeout)
            .idle_timeout(settings.idle_timeout);
        if sqlite_is_memory(url) {
            pool_options = pool_options
                .min_connections(0)
                .max_connections(1)
                .idle_timeout(None)
                .max_lifetime(None);
        }

        let pool = pool_options
            .connect_with(options)
            .await
            .map_err(|error| OrmError::Pool(error.to_string()))?;
        Ok(DbPool::Sqlite(pool))
    }

    /// Report that the SQLite driver is not compiled in.
    #[cfg(not(feature = "sqlite"))]
    async fn connect_sqlite(_url: &str, _settings: PoolSettings) -> Result<Self> {
        Err(OrmError::UnsupportedDriver("sqlite".to_string()))
    }

    /// Connect a Postgres pool.
    #[cfg(feature = "postgres")]
    async fn connect_postgres(url: &str, settings: PoolSettings) -> Result<Self> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .min_connections(settings.min_connections)
            .max_connections(settings.max_connections)
            .acquire_timeout(settings.acquire_timeout)
            .idle_timeout(settings.idle_timeout)
            .connect(url)
            .await
            .map_err(|error| OrmError::Pool(error.to_string()))?;
        Ok(DbPool::Postgres(pool))
    }

    /// Report that the Postgres driver is not compiled in.
    #[cfg(not(feature = "postgres"))]
    async fn connect_postgres(_url: &str, _settings: PoolSettings) -> Result<Self> {
        Err(OrmError::UnsupportedDriver("postgres".to_string()))
    }

    /// Connect a MySQL pool.
    #[cfg(feature = "mysql")]
    async fn connect_mysql(url: &str, settings: PoolSettings) -> Result<Self> {
        let pool = sqlx::mysql::MySqlPoolOptions::new()
            .min_connections(settings.min_connections)
            .max_connections(settings.max_connections)
            .acquire_timeout(settings.acquire_timeout)
            .idle_timeout(settings.idle_timeout)
            .connect(url)
            .await
            .map_err(|error| OrmError::Pool(error.to_string()))?;
        Ok(DbPool::MySql(pool))
    }

    /// Report that the MySQL driver is not compiled in.
    #[cfg(not(feature = "mysql"))]
    async fn connect_mysql(_url: &str, _settings: PoolSettings) -> Result<Self> {
        Err(OrmError::UnsupportedDriver("mysql".to_string()))
    }
}

/// Whether a SQLite URL addresses an in-memory database.
#[cfg(feature = "sqlite")]
fn sqlite_is_memory(url: &str) -> bool {
    url.contains(":memory:") || url.contains("mode=memory")
}

/// Supported database engines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Driver {
    /// SQLite (file-backed or in-memory).
    Sqlite,
    /// PostgreSQL.
    Postgres,
    /// MySQL / MariaDB.
    MySql,
}

impl Driver {
    /// Map a `driver` string (with common aliases) onto a [`Driver`].
    pub(crate) fn from_name(name: &str) -> Result<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "sqlite" => Ok(Driver::Sqlite),
            "postgres" | "postgresql" | "pg" => Ok(Driver::Postgres),
            "mysql" | "mariadb" => Ok(Driver::MySql),
            _ => Err(OrmError::Connection(ConnectionError::UnsupportedDriver {
                driver: name.to_string(),
            })),
        }
    }

    /// Map a URL scheme onto a [`Driver`], or `None` when unrecognised.
    fn from_scheme(scheme: &str) -> Option<Self> {
        match scheme.to_ascii_lowercase().as_str() {
            "sqlite" => Some(Driver::Sqlite),
            "postgres" | "postgresql" => Some(Driver::Postgres),
            "mysql" | "mariadb" => Some(Driver::MySql),
            _ => None,
        }
    }

    /// The canonical URL scheme for this driver.
    fn scheme(self) -> &'static str {
        match self {
            Driver::Sqlite => "sqlite",
            Driver::Postgres => "postgres",
            Driver::MySql => "mysql",
        }
    }

    /// The default TCP port for network drivers.
    fn default_port(self) -> u16 {
        match self {
            Driver::Sqlite => 0,
            Driver::Postgres => 5432,
            Driver::MySql => 3306,
        }
    }
}

/// Extract the scheme from a URL (`sqlite://…` → `sqlite`).
pub(crate) fn scheme_of(url: &str) -> &str {
    url.split(':').next().unwrap_or_default()
}

/// Trim `value` and require it non-empty, else a typed missing-field error.
fn required<'a>(field: &str, value: Option<&'a str>) -> Result<&'a str> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            OrmError::Connection(ConnectionError::MissingField {
                field: field.to_string(),
            })
        })
}

/// Percent-encode a `username`/`password` userinfo component (RFC 3986).
/// Reserved delimiters (`@ : / ? # [ ] %` …) are escaped so credentials cannot
/// corrupt the URL authority; unreserved `A-Z a-z 0-9 -._~` stay literal.
pub(crate) fn encode_userinfo(value: &str) -> String {
    use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
    const USERINFO: &AsciiSet = &NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'.')
        .remove(b'_')
        .remove(b'~');
    utf8_percent_encode(value, USERINFO).to_string()
}

impl ConnectionConfig {
    /// Build a driver-valid URL for the write (primary) endpoint.
    ///
    /// Applies the optional `[….write]` overlay, then returns its `url` after
    /// validating the scheme agrees with `driver`; otherwise the URL is
    /// assembled from the granular fields.
    ///
    /// # Errors
    ///
    /// A typed [`OrmError::Connection`] for an unsupported driver, a missing
    /// required field, or a `driver`/URL-scheme disagreement.
    pub fn build_url(&self) -> Result<String> {
        self.effective(self.write.as_ref()).assemble()
    }

    /// Build the read-endpoint URL (`[….read]` overlay, else the write URL).
    ///
    /// The overlay inherits the primary fields, so a replica sharing credentials
    /// only needs its own `host`; with no `read` table the write URL is returned
    /// and the resolver reuses one pool for both roles. Errors propagate from
    /// [`ConnectionConfig::build_url`] on the overlay.
    pub fn read_url(&self) -> Result<String> {
        if self.read.is_some() {
            self.effective(self.read.as_ref()).assemble()
        } else {
            self.build_url()
        }
    }
    /// Overlay `endpoint` (when present) onto the primary fields; endpoint
    /// fields win where set, every omitted field inherits the primary.
    fn effective(&self, endpoint: Option<&EndpointConfig>) -> ConnectionConfig {
        let Some(endpoint) = endpoint else {
            return self.clone();
        };
        ConnectionConfig {
            driver: self.driver.clone(),
            url: endpoint.url.clone(),
            host: endpoint.host.clone().or_else(|| self.host.clone()),
            port: endpoint.port.or(self.port),
            database: endpoint.database.clone().or_else(|| self.database.clone()),
            username: endpoint.username.clone().or_else(|| self.username.clone()),
            password: endpoint.password.clone().or_else(|| self.password.clone()),
            charset: endpoint.charset.clone().or_else(|| self.charset.clone()),
            read: None,
            write: None,
            pool: self.pool.clone(),
        }
    }

    /// Assemble the URL from the already-overlaid fields (no further fallback).
    fn assemble(&self) -> Result<String> {
        let driver = Driver::from_name(&self.driver)?;

        if let Some(url) = self.url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
            let scheme = scheme_of(url);
            match Driver::from_scheme(scheme) {
                Some(url_driver) if url_driver == driver => {}
                Some(_) => {
                    return Err(OrmError::Connection(ConnectionError::DriverMismatch {
                        driver: self.driver.clone(),
                        scheme: scheme.to_string(),
                    }))
                }
                None => {
                    return Err(OrmError::Connection(ConnectionError::UnsupportedDriver {
                        driver: scheme.to_string(),
                    }))
                }
            }
            return Ok(url.to_string());
        }

        match driver {
            Driver::Sqlite => self.build_sqlite(),
            Driver::Postgres | Driver::MySql => self.build_network(driver),
        }
    }

    /// Assemble a `sqlite:` URL from the `database` field.
    fn build_sqlite(&self) -> Result<String> {
        let database = required("database", self.database.as_deref())?;
        if database == ":memory:" || database.contains("mode=memory") {
            return Ok("sqlite::memory:".to_string());
        }
        Ok(format!("sqlite://{database}"))
    }

    /// Assemble a `postgres://`/`mysql://` URL from the granular fields; the
    /// `username`/`password` are percent-encoded so reserved characters in
    /// credentials cannot corrupt the URL authority.
    fn build_network(&self, driver: Driver) -> Result<String> {
        let host = required("host", self.host.as_deref())?;
        let database = required("database", self.database.as_deref())?;
        let port = self.port.unwrap_or_else(|| driver.default_port());

        let mut url = format!("{}://", driver.scheme());
        if let Some(username) = self.username.as_deref().filter(|u| !u.is_empty()) {
            url.push_str(&encode_userinfo(username));
            if let Some(password) = self.password.as_deref().filter(|p| !p.is_empty()) {
                url.push(':');
                url.push_str(&encode_userinfo(password));
            }
            url.push('@');
        }
        url.push_str(host);
        url.push(':');
        url.push_str(&port.to_string());
        url.push('/');
        url.push_str(database);
        if let Some(charset) = self.charset.as_deref().filter(|c| !c.is_empty()) {
            url.push_str("?charset=");
            url.push_str(charset);
        }
        Ok(url)
    }
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use super::*;

    /// Verifies an in-memory SQLite pool connects and answers a ping.
    #[tokio::test]
    async fn connects_and_pings_sqlite_memory() {
        let pool = DbPool::connect("sqlite::memory:").await.expect("connect");
        assert_eq!(pool.dialect(), "sqlite");
        assert!(pool.sqlite().is_some());
        pool.ping().await.expect("ping");
        pool.close().await;
    }

    /// Verifies in-memory pools are pinned to a single shared connection.
    #[tokio::test]
    async fn sqlite_memory_pool_is_single_connection() {
        let pool = DbPool::connect("sqlite::memory:").await.unwrap();
        match &pool {
            DbPool::Sqlite(inner) => assert_eq!(inner.options().get_max_connections(), 1),
            #[cfg(feature = "postgres")]
            DbPool::Postgres(_) => {}
            #[cfg(feature = "mysql")]
            DbPool::MySql(_) => {}
        }
        pool.close().await;
    }

    /// Verifies an invalid URL maps to a typed pool error.
    #[tokio::test]
    async fn invalid_url_maps_to_pool_error() {
        let error = DbPool::connect("nonsense://localhost/none")
            .await
            .unwrap_err();
        assert!(matches!(error, OrmError::Pool(_)), "got {error:?}");
    }
}
