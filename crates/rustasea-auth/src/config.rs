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
//!
//! ## Environment bridge
//!
//! The loader's `__` separator means single-underscore variables never reach the
//! nested `[auth]` / `[session]` tables, so the documented `AUTH_*` and
//! `SESSION_*` names are read directly by [`AuthConfig::apply_env`] and
//! [`SessionConfig::apply_env`] (the only path for `.env` parity).
use std::collections::BTreeMap;

use serde::Deserialize;

use rustasea_config::ConfigLoader;

use crate::error::AuthConfigError;

mod fortify;
mod session;

pub use fortify::{
    FortifyConfig, FortifyFeaturesConfig, FortifyLimiterConfig, FortifyPasskeyFeatureConfig,
    FortifyPasskeysConfig, FortifyTwoFactorConfig, ResolvedPasskeys,
};
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
pub const DEFAULT_PASSWORD_TIMEOUT_SECS: u64 = 10_800;

/// Default `password_timeout` (3 hours, Laravel parity).
fn default_password_timeout() -> u64 {
    DEFAULT_PASSWORD_TIMEOUT_SECS
}

/// Resolve the password-confirmation window in seconds for runtime gates.
///
/// Reads `AUTH_PASSWORD_TIMEOUT` (the documented environment override) and falls
/// back to [`DEFAULT_PASSWORD_TIMEOUT_SECS`] when the variable is unset, blank,
/// or not a non-negative integer. This is the runtime counterpart of
/// [`AuthConfig::password_timeout`]: middleware that runs without a loaded
/// [`AuthConfig`] (the app's `require_password_confirmed` gate) can still honour
/// the configured window without threading the whole config through `AppState`.
/// A malformed value falls back to the default rather than panicking, matching
/// the fail-closed posture of the other gates.
pub fn password_timeout_secs() -> u64 {
    env_non_empty("AUTH_PASSWORD_TIMEOUT")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_PASSWORD_TIMEOUT_SECS)
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
    /// Deserialize `[auth]` from a layered [`ConfigLoader`] and apply the
    /// documented environment overrides.
    ///
    /// A missing `[auth]` table yields [`AuthConfig::default`] (tolerated,
    /// matching the loader's missing-file policy); a table that exists but does
    /// not deserialize surfaces [`AuthConfigError::Invalid`].
    ///
    /// # Errors
    ///
    /// [`AuthConfigError::Invalid`] when `[auth]` exists but is malformed, or
    /// when a documented `AUTH_*` override is not a non-negative integer.
    pub fn from_loader(loader: &ConfigLoader) -> ConfigResult<Self> {
        let mut config = match loader.get_key::<AuthConfig>("auth") {
            Ok(config) => config,
            Err(error) => {
                if loader.inner().get_table("auth").is_err() {
                    AuthConfig::default()
                } else {
                    return Err(AuthConfigError::Invalid(error.to_string()));
                }
            }
        };
        config.apply_env()?;
        Ok(config)
    }

    /// Apply the documented single-underscore `AUTH_*` environment overrides.
    ///
    /// `AUTH_GUARD` replaces `auth.defaults.guard`, `AUTH_PASSWORD_BROKER`
    /// replaces `auth.defaults.passwords`, `AUTH_MODEL` replaces the `users`
    /// provider's model (`auth.providers.users.model`),
    /// `AUTH_PASSWORD_RESET_TOKEN_TABLE` replaces the `users` broker's table
    /// (`auth.passwords.users.table`), and `AUTH_PASSWORD_TIMEOUT` replaces the
    /// top-level `password_timeout` (seconds). The environment wins over the
    /// file and a blank value is ignored; the loader's `__` separator means
    /// single-underscore variables never reach the nested `[auth]` table, so
    /// this bridge is the only path for `.env` parity.
    ///
    /// # Errors
    ///
    /// [`AuthConfigError::Invalid`] when `AUTH_PASSWORD_TIMEOUT` is not a
    /// non-negative integer.
    pub fn apply_env(&mut self) -> ConfigResult<()> {
        if let Some(guard) = env_non_empty("AUTH_GUARD") {
            self.defaults.guard = guard;
        }
        if let Some(broker) = env_non_empty("AUTH_PASSWORD_BROKER") {
            self.defaults.passwords = broker;
        }
        if let Some(model) = env_non_empty("AUTH_MODEL") {
            self.providers.entry("users".to_string()).or_default().model = Some(model);
        }
        if let Some(table) = env_non_empty("AUTH_PASSWORD_RESET_TOKEN_TABLE") {
            self.passwords.entry("users".to_string()).or_default().table = table;
        }
        if let Some(timeout) = env_non_empty("AUTH_PASSWORD_TIMEOUT") {
            self.password_timeout = timeout.parse::<u64>().map_err(|_| {
                AuthConfigError::Invalid(format!(
                    "AUTH_PASSWORD_TIMEOUT {timeout:?} is not a non-negative integer"
                ))
            })?;
        }
        Ok(())
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

/// Read an environment variable, treating unset or blank values as absent.
fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// Serializes tests that read or mutate process-global `AUTH_*` / `SESSION_*`
/// variables across the `config` test modules.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests;
