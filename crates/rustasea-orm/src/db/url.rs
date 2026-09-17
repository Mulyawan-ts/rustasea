//! Driver selection and connection-URL assembly for the ORM `[database]` config.
//!
//! [`ConnectionConfig::build_url`] and [`ConnectionConfig::read_url`] resolve the
//! primary and replica endpoints; [`Driver`] maps the configured driver name and
//! a URL scheme onto a supported engine. Granular fields are assembled into a
//! driver-valid URL, with `username`/`password` percent-encoded so reserved
//! characters cannot corrupt the URL authority.

use crate::connections::{ConnectionConfig, EndpointConfig};
use crate::error::{ConnectionError, OrmError, Result};

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
