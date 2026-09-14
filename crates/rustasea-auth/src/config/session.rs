//! Typed `[session]` table (Laravel 13.x `config/session.php` parity).
//!
//! [`SessionConfig`] mirrors Laravel's `config/session.php`. RustaSea implements
//! two backing stores: `memory`, the tower-sessions `MemoryStore` (per-process,
//! the default), and `database`, a [`crate::session::DatabaseSessionStore`] over
//! the ORM connection pool (selected by `connection` + `table`). The remaining
//! Laravel drivers (`redis`, `file`, `cookie`, `array`) are recognised by name
//! but **not yet implemented**; selecting one is a typed
//! [`AuthConfigError::UnsupportedSessionDriver`] at load time rather than a
//! silent fallback, so a misconfigured production driver can never degrade into
//! a store that drops sessions on restart.
//!
//! [`SessionConfig::from_loader`] deserializes the table from a layered
//! [`rustasea_config::ConfigLoader`], applies the documented single-underscore
//! `SESSION_*` environment overrides via [`SessionConfig::apply_env`], and then
//! validates the selected driver. The mapping helpers
//! [`SessionConfig::to_cookie_config`] and [`SessionConfig::to_policy`] project
//! the typed config onto the guard's runtime types, and
//! [`SessionConfig::ttl_secs`] converts the Laravel-style minute `lifetime` into
//! the seconds the guard advertises on issued tokens.

use serde::Deserialize;
use tower_sessions::cookie::SameSite;

use rustasea_config::ConfigLoader;

use crate::error::AuthConfigError;
use crate::session::SessionPolicy;
use crate::session_cookie::SessionCookieConfig;

use super::ConfigResult;

/// Default session driver (`memory`; the default store).
fn default_session_driver() -> String {
    "memory".to_string()
}

/// Default session lifetime in minutes (2 hours, Laravel parity).
fn default_lifetime_minutes() -> u64 {
    120
}

/// Default file-session directory (parity; unused by the memory store).
fn default_files() -> String {
    "storage/framework/sessions".to_string()
}

/// Default connection name for database/redis drivers (parity).
fn default_connection() -> String {
    "default".to_string()
}

/// Default session table for the database driver (parity).
fn default_table() -> String {
    "sessions".to_string()
}

/// Default named store within a driver (parity).
fn default_store() -> String {
    "default".to_string()
}

/// Default GC lottery `[chance, out_of]` (Laravel `[2, 100]`).
fn default_lottery() -> [u64; 2] {
    [2, 100]
}

/// Default cookie name; the `-session-` marker is required by the policy.
fn default_cookie_name() -> String {
    "rustasea-session".to_string()
}

/// Default cookie path.
fn default_path() -> String {
    "/".to_string()
}

/// Default `HttpOnly` flag (hidden from JavaScript).
fn default_http_only() -> bool {
    true
}

/// Default `SameSite` policy string (`lax`).
fn default_same_site() -> String {
    "lax".to_string()
}

/// Default serialization format (`json`; the only supported format).
fn default_serialization() -> String {
    "json".to_string()
}

/// Typed `[session]` table (Laravel 13.x `config/session.php` shape).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SessionConfig {
    /// Backing store; `memory` (default) or `database`.
    #[serde(default = "default_session_driver")]
    pub driver: String,
    /// Lifetime in **minutes** (converted to seconds by [`SessionConfig::ttl_secs`]).
    #[serde(default = "default_lifetime_minutes", rename = "lifetime")]
    pub lifetime_minutes: u64,
    /// Destroy the session when the browser closes.
    #[serde(default)]
    pub expire_on_close: bool,
    /// Encrypt the session payload at rest (**not implemented**).
    ///
    /// Reserved for parity with Laravel: neither the `memory` nor the
    /// `database` store encrypts its payload (the database store writes
    /// plaintext JSON to the `payload` column), so setting this `true` has no
    /// effect.
    #[serde(default)]
    pub encrypt: bool,
    /// Directory for the (unimplemented) file driver.
    #[serde(default = "default_files")]
    pub files: String,
    /// Connection name for the database driver (`redis` unimplemented).
    #[serde(default = "default_connection")]
    pub connection: String,
    /// Table for the database driver.
    #[serde(default = "default_table")]
    pub table: String,
    /// Named store within a driver (parity).
    #[serde(default = "default_store")]
    pub store: String,
    /// GC lottery `[chance, out_of]`.
    #[serde(default = "default_lottery")]
    pub lottery: [u64; 2],
    /// Cookie name (`session.cookie`); must contain the `-session-` marker.
    #[serde(default = "default_cookie_name", rename = "cookie")]
    pub cookie_name: String,
    /// Cookie path scope.
    #[serde(default = "default_path")]
    pub path: String,
    /// Cookie domain; empty string means host-only.
    #[serde(default)]
    pub domain: Option<String>,
    /// Send the cookie over HTTPS only.
    #[serde(default)]
    pub secure: bool,
    /// Hide the cookie from JavaScript.
    #[serde(default = "default_http_only")]
    pub http_only: bool,
    /// `SameSite` policy string: `lax` | `strict` | `none`.
    #[serde(default = "default_same_site")]
    pub same_site: String,
    /// Partitioned (CHIPS) cookie flag.
    #[serde(default)]
    pub partitioned: bool,
    /// Session payload serialization; `json` is the only supported format.
    #[serde(default = "default_serialization")]
    pub serialization: String,
}

impl Default for SessionConfig {
    /// RustaSea defaults: memory store, 120-minute lifetime, hardened cookie.
    fn default() -> Self {
        Self {
            driver: default_session_driver(),
            lifetime_minutes: default_lifetime_minutes(),
            expire_on_close: false,
            encrypt: false,
            files: default_files(),
            connection: default_connection(),
            table: default_table(),
            store: default_store(),
            lottery: default_lottery(),
            cookie_name: default_cookie_name(),
            path: default_path(),
            domain: None,
            secure: false,
            http_only: default_http_only(),
            same_site: default_same_site(),
            partitioned: false,
            serialization: default_serialization(),
        }
    }
}

impl SessionConfig {
    /// Deserialize `[session]` from a layered [`ConfigLoader`], apply the
    /// documented environment overrides, and validate the selected driver.
    ///
    /// A missing `[session]` table yields [`SessionConfig::default`]; a table
    /// that exists but is malformed surfaces [`AuthConfigError::Invalid`], a
    /// malformed `SESSION_*` override surfaces [`AuthConfigError::Invalid`], and
    /// a table selecting an unimplemented driver surfaces
    /// [`AuthConfigError::UnsupportedSessionDriver`].
    ///
    /// # Errors
    ///
    /// [`AuthConfigError::Invalid`] on malformed TOML or an unparsable
    /// `SESSION_LIFETIME` / boolean override, or
    /// [`AuthConfigError::UnsupportedSessionDriver`] when `driver` is neither
    /// `memory` nor `database`.
    pub fn from_loader(loader: &ConfigLoader) -> ConfigResult<Self> {
        let mut config = match loader.get_key::<SessionConfig>("session") {
            Ok(config) => config,
            Err(error) => {
                if loader.inner().get_table("session").is_err() {
                    SessionConfig::default()
                } else {
                    return Err(AuthConfigError::Invalid(error.to_string()));
                }
            }
        };
        config.apply_env()?;
        config.validate_driver()?;
        Ok(config)
    }

    /// Apply the documented single-underscore `SESSION_*` environment overrides.
    ///
    /// `SESSION_DRIVER` replaces the backing store, `SESSION_LIFETIME` the
    /// lifetime in minutes, `SESSION_COOKIE` the cookie name, `SESSION_SECURE`
    /// the HTTPS-only flag, and `SESSION_SAME_SITE` the `SameSite` policy.
    /// `SESSION_EXPIRE_ON_CLOSE`, `SESSION_ENCRYPT`, `SESSION_PARTITIONED_COOKIE`,
    /// and `SESSION_HTTP_ONLY` replace their boolean counterparts;
    /// `SESSION_CONNECTION`, `SESSION_TABLE`, `SESSION_STORE`, `SESSION_PATH`,
    /// and `SESSION_DOMAIN` replace their string counterparts. The environment
    /// wins over the file and a blank value is ignored; the loader's `__`
    /// separator means single-underscore variables never reach the nested
    /// `[session]` table, so this bridge is the only path for `.env` parity.
    ///
    /// `SESSION_SECURE_COOKIE` is accepted as an alias of `SESSION_SECURE` for
    /// Laravel / starter-kit parity; when both are set the explicit
    /// `SESSION_SECURE` wins.
    ///
    /// # Errors
    ///
    /// [`AuthConfigError::Invalid`] when `SESSION_LIFETIME` is not a
    /// non-negative integer or any boolean override (`SESSION_SECURE`,
    /// `SESSION_SECURE_COOKIE`, `SESSION_EXPIRE_ON_CLOSE`, `SESSION_ENCRYPT`,
    /// `SESSION_PARTITIONED_COOKIE`, `SESSION_HTTP_ONLY`) is not a recognised
    /// boolean.
    pub fn apply_env(&mut self) -> ConfigResult<()> {
        if let Some(driver) = env_non_empty("SESSION_DRIVER") {
            self.driver = driver;
        }
        if let Some(lifetime) = env_non_empty("SESSION_LIFETIME") {
            self.lifetime_minutes = lifetime.parse::<u64>().map_err(|_| {
                AuthConfigError::Invalid(format!(
                    "SESSION_LIFETIME {lifetime:?} is not a non-negative integer"
                ))
            })?;
        }
        if let Some(cookie) = env_non_empty("SESSION_COOKIE") {
            self.cookie_name = cookie;
        }
        if let Some(secure) = env_non_empty("SESSION_SECURE") {
            self.secure = parse_bool("SESSION_SECURE", &secure)?;
        } else if let Some(secure) = env_non_empty("SESSION_SECURE_COOKIE") {
            self.secure = parse_bool("SESSION_SECURE_COOKIE", &secure)?;
        }
        if let Some(same_site) = env_non_empty("SESSION_SAME_SITE") {
            self.same_site = same_site;
        }
        if let Some(expire_on_close) = env_non_empty("SESSION_EXPIRE_ON_CLOSE") {
            self.expire_on_close = parse_bool("SESSION_EXPIRE_ON_CLOSE", &expire_on_close)?;
        }
        if let Some(encrypt) = env_non_empty("SESSION_ENCRYPT") {
            self.encrypt = parse_bool("SESSION_ENCRYPT", &encrypt)?;
        }
        if let Some(partitioned) = env_non_empty("SESSION_PARTITIONED_COOKIE") {
            self.partitioned = parse_bool("SESSION_PARTITIONED_COOKIE", &partitioned)?;
        }
        if let Some(http_only) = env_non_empty("SESSION_HTTP_ONLY") {
            self.http_only = parse_bool("SESSION_HTTP_ONLY", &http_only)?;
        }
        if let Some(connection) = env_non_empty("SESSION_CONNECTION") {
            self.connection = connection;
        }
        if let Some(table) = env_non_empty("SESSION_TABLE") {
            self.table = table;
        }
        if let Some(store) = env_non_empty("SESSION_STORE") {
            self.store = store;
        }
        if let Some(path) = env_non_empty("SESSION_PATH") {
            self.path = path;
        }
        if let Some(domain) = env_non_empty("SESSION_DOMAIN") {
            self.domain = Some(domain);
        }
        Ok(())
    }

    /// Accept the config only when `driver` is an implemented store.
    ///
    /// `memory` (the default) and `database` are implemented; any other driver —
    /// including the recognised-but-unimplemented `redis`/`file`/`cookie`/
    /// `array` — is rejected so a misconfigured production driver fails closed at
    /// load time rather than degrading into a store that drops sessions.
    ///
    /// # Errors
    ///
    /// [`AuthConfigError::UnsupportedSessionDriver`] for any driver other than
    /// `memory` or `database`.
    pub fn validate_driver(&self) -> ConfigResult<()> {
        if self.is_memory_driver() || self.is_database_driver() {
            Ok(())
        } else {
            Err(AuthConfigError::UnsupportedSessionDriver(
                self.driver.clone(),
            ))
        }
    }

    /// Whether the selected driver is the in-memory store (`memory`).
    pub fn is_memory_driver(&self) -> bool {
        self.driver.eq_ignore_ascii_case("memory")
    }

    /// Whether the selected driver is the database store (`database`).
    ///
    /// When `true`, build the guard with the async
    /// [`SessionGuard::from_config_database`](crate::session::SessionGuard::from_config_database);
    /// otherwise use the in-memory
    /// [`SessionGuard::from_config`](crate::session::SessionGuard::from_config).
    pub fn is_database_driver(&self) -> bool {
        self.driver.eq_ignore_ascii_case("database")
    }

    /// Lifetime in seconds (`lifetime_minutes * 60`).
    pub fn ttl_secs(&self) -> u64 {
        self.lifetime_minutes.saturating_mul(60)
    }

    /// The cookie domain, treating an empty string as host-only (`None`).
    pub fn normalized_domain(&self) -> Option<String> {
        self.domain
            .as_deref()
            .map(str::trim)
            .filter(|domain| !domain.is_empty())
            .map(str::to_string)
    }

    /// Parse `same_site` into the cookie crate's enum.
    ///
    /// # Errors
    ///
    /// [`AuthConfigError::InvalidSameSite`] when the value is not `lax`,
    /// `strict`, or `none` (case-insensitive).
    pub fn same_site(&self) -> ConfigResult<SameSite> {
        match self.same_site.trim().to_ascii_lowercase().as_str() {
            "lax" => Ok(SameSite::Lax),
            "strict" => Ok(SameSite::Strict),
            "none" => Ok(SameSite::None),
            _ => Err(AuthConfigError::InvalidSameSite(self.same_site.clone())),
        }
    }

    /// Key prefix derived from the cookie name.
    ///
    /// A single trailing `-` is guaranteed, so `rustasea-session` becomes
    /// `rustasea-session-` and satisfies the `-session-` marker rule.
    pub fn policy_prefix(&self) -> String {
        format!("{}-", self.cookie_name.trim_end_matches('-'))
    }

    /// Project the config onto the guard's [`SessionCookieConfig`].
    ///
    /// # Errors
    ///
    /// [`AuthConfigError::InvalidSameSite`] when `same_site` is unrecognised.
    pub fn to_cookie_config(&self) -> ConfigResult<SessionCookieConfig> {
        Ok(SessionCookieConfig {
            name: self.cookie_name.clone(),
            http_only: self.http_only,
            secure: self.secure,
            same_site: self.same_site()?,
            path: self.path.clone(),
            domain: self.normalized_domain(),
        })
    }

    /// Project the config onto the guard's [`SessionPolicy`].
    ///
    /// # Errors
    ///
    /// [`AuthConfigError::UnsupportedSerialization`] when `serialization` is
    /// not `json`, or [`AuthConfigError::Invalid`] when the cookie name yields a
    /// prefix without the `-session-` marker.
    pub fn to_policy(&self) -> ConfigResult<SessionPolicy> {
        if self.serialization != "json" {
            return Err(AuthConfigError::UnsupportedSerialization(
                self.serialization.clone(),
            ));
        }
        let prefix = self.policy_prefix();
        if !prefix.contains("-session-") {
            return Err(AuthConfigError::Invalid(format!(
                "session cookie name {:?} yields prefix {:?} without a -session- marker",
                self.cookie_name, prefix
            )));
        }
        Ok(SessionPolicy {
            serialization: self.serialization.clone(),
            prefix,
            serializable_classes: Vec::new(),
        })
    }
}

/// Read an environment variable, treating unset or blank values as absent.
fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// Parse a truthy/falsy environment override, mapping an unrecognised value to a
/// typed [`AuthConfigError::Invalid`] that names the offending variable.
fn parse_bool(key: &str, value: &str) -> ConfigResult<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Ok(true),
        "0" | "false" | "off" | "no" => Ok(false),
        _ => Err(AuthConfigError::Invalid(format!(
            "{key} {value:?} is not a recognised boolean"
        ))),
    }
}

#[cfg(test)]
mod tests;
