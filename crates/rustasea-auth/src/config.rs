//! Typed `auth.toml` / `session.toml` configuration (Laravel 13.x parity).
//!
//! This module owns parsing of the `[auth]` and `[session]` tables (read through
//! [`rustasea_config::ConfigLoader`]) and turns them into typed, validated
//! structs plus the wiring helpers the session guard consumes.
//!
//! ## `[auth]`
//!
//! Mirrors `config/auth.php`:
//!
//! * [`AuthDefaults`] — `guard` / `passwords` selectors.
//! * [`GuardConfig`] — one entry per `[auth.guards.<name>]` (`driver`, `provider`).
//! * [`ProviderConfig`] — one entry per `[auth.providers.<name>]` (`driver`, `model`).
//! * [`PasswordBrokerConfig`] — one entry per `[auth.passwords.<name>]`.
//! * `password_timeout` — top-level scalar (seconds).
//!
//! ## `[session]`
//!
//! Mirrors `config/session.php`. RustaSea implements a single backing store:
//!
//! * `memory` — the default; the tower-sessions `MemoryStore` (per-process).
//!
//! The remaining Laravel drivers (`redis`, `database`, `file`, `cookie`,
//! `array`) are recognised by name but **not yet implemented**; selecting one
//! is a typed [`AuthConfigError::UnsupportedSessionDriver`] at load time rather
//! than a silent fallback, so a misconfigured production driver can never
//! degrade into a store that drops sessions on restart.
//!
//! ## Mapping
//!
//! [`SessionConfig::to_cookie_config`] and [`SessionConfig::to_policy`] project
//! the typed config onto the guard's runtime types, and [`SessionConfig::ttl_secs`]
//! converts the Laravel-style minute `lifetime` into the seconds the guard
//! advertises on issued tokens.
use std::collections::BTreeMap;

use serde::Deserialize;

use rustasea_config::ConfigLoader;

use crate::error::AuthConfigError;

mod session;

pub use session::SessionConfig;

/// Result alias for configuration parsing / validation.
pub type ConfigResult<T> = std::result::Result<T, AuthConfigError>;

// ---------------------------------------------------------------------------
// auth.toml
// ---------------------------------------------------------------------------

/// Default guard name used when `auth.defaults.guard` is omitted.
fn default_guard_name() -> String {
    "web".to_string()
}

/// Default password broker used when `auth.defaults.passwords` is omitted.
fn default_password_broker() -> String {
    "users".to_string()
}

/// Default `password_timeout` (3 hours, Laravel parity).
fn default_password_timeout() -> u64 {
    10_800
}

/// `auth.defaults` — the active guard and password broker.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AuthDefaults {
    /// Guard used when no guard name is requested.
    #[serde(default = "default_guard_name")]
    pub guard: String,
    /// Password broker used by the reset flow.
    #[serde(default = "default_password_broker")]
    pub passwords: String,
}

impl Default for AuthDefaults {
    /// Laravel defaults: `web` guard, `users` broker.
    fn default() -> Self {
        Self {
            guard: default_guard_name(),
            passwords: default_password_broker(),
        }
    }
}

/// One `[auth.guards.<name>]` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct GuardConfig {
    /// Guard driver (`session`, `token`, …).
    #[serde(default)]
    pub driver: String,
    /// Provider name; `None` for guards that do not read a user provider.
    #[serde(default)]
    pub provider: Option<String>,
}

/// One `[auth.providers.<name>]` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ProviderConfig {
    /// Provider driver (`eloquent`, …).
    #[serde(default)]
    pub driver: String,
    /// Fully-qualified model path (kept verbatim; a string for parity).
    #[serde(default)]
    pub model: Option<String>,
}

/// One `[auth.passwords.<name>]` password-reset broker entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct PasswordBrokerConfig {
    /// Provider backing the broker.
    #[serde(default)]
    pub provider: String,
    /// Reset-token table.
    #[serde(default)]
    pub table: String,
    /// Minutes a reset token stays valid.
    #[serde(default)]
    pub expire: u64,
    /// Seconds between reset attempts.
    #[serde(default)]
    pub throttle: u64,
}

/// Typed `[auth]` table.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AuthConfig {
    /// `[auth.defaults]` selectors.
    #[serde(default)]
    pub defaults: AuthDefaults,
    /// Named guards, ordered for deterministic iteration.
    #[serde(default)]
    pub guards: BTreeMap<String, GuardConfig>,
    /// Named user providers.
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderConfig>,
    /// Named password-reset brokers.
    #[serde(default)]
    pub passwords: BTreeMap<String, PasswordBrokerConfig>,
    /// Seconds before a sensitive action re-confirms the password.
    #[serde(default = "default_password_timeout")]
    pub password_timeout: u64,
}

impl Default for AuthConfig {
    /// Laravel-shaped defaults with no declared guards/providers.
    fn default() -> Self {
        Self {
            defaults: AuthDefaults::default(),
            guards: BTreeMap::new(),
            providers: BTreeMap::new(),
            passwords: BTreeMap::new(),
            password_timeout: default_password_timeout(),
        }
    }
}

impl AuthConfig {
    /// Deserialize `[auth]` from a layered [`ConfigLoader`].
    ///
    /// A missing `[auth]` table yields [`AuthConfig::default`] (tolerated,
    /// matching the loader's missing-file policy); a table that exists but does
    /// not deserialize surfaces [`AuthConfigError::Invalid`].
    ///
    /// # Errors
    ///
    /// [`AuthConfigError::Invalid`] when `[auth]` exists but is malformed.
    pub fn from_loader(loader: &ConfigLoader) -> ConfigResult<Self> {
        match loader.get_key::<AuthConfig>("auth") {
            Ok(config) => Ok(config),
            Err(error) => {
                if loader.inner().get_table("auth").is_err() {
                    Ok(AuthConfig::default())
                } else {
                    Err(AuthConfigError::Invalid(error.to_string()))
                }
            }
        }
    }

    /// Name of the default guard (`auth.defaults.guard`).
    pub fn default_guard(&self) -> &str {
        &self.defaults.guard
    }

    /// Name of the default password broker (`auth.defaults.passwords`).
    pub fn default_password_broker(&self) -> &str {
        &self.defaults.passwords
    }

    /// Look up a guard declaration by name.
    pub fn guard_config(&self, name: &str) -> Option<&GuardConfig> {
        self.guards.get(name)
    }

    /// Resolve the [`ProviderConfig`] a guard reads users from.
    ///
    /// # Errors
    ///
    /// [`AuthConfigError::UnknownGuard`] when `guard` is undeclared, or
    /// [`AuthConfigError::MissingGuardProvider`] when the guard names no
    /// provider or names one that is not declared under `[auth.providers]`.
    pub fn provider_for(&self, guard: &str) -> ConfigResult<&ProviderConfig> {
        let guard_config = self
            .guards
            .get(guard)
            .ok_or_else(|| AuthConfigError::UnknownGuard(guard.to_string()))?;
        let provider = guard_config.provider.as_deref().unwrap_or("");
        if provider.is_empty() {
            return Err(AuthConfigError::MissingGuardProvider {
                guard: guard.to_string(),
                provider: String::new(),
            });
        }
        self.providers
            .get(provider)
            .ok_or_else(|| AuthConfigError::MissingGuardProvider {
                guard: guard.to_string(),
                provider: provider.to_string(),
            })
    }
}

#[cfg(test)]
mod tests;
