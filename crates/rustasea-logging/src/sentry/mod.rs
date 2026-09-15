//! Sentry error-tracking integration (ADOPT-004) — opt-in, inert until a DSN
//! is configured.
//!
//! This module mirrors the observability surface of `sentry/sentry-laravel`: it
//! reads the `[logging.sentry]` table (plus the `SENTRY_*` environment
//! overrides), builds a [`sentry::ClientOptions`] from it, and installs the
//! process-global Sentry client. The [`tracing_layer`] helper lets the
//! subscriber composer ([`crate::init`]) forward `tracing` events to Sentry, and
//! [`scrub_event`] is the `before_send` hook that removes credentials before an
//! event leaves the process.
//!
//! # Disabled by default
//!
//! Nothing here has any effect until a DSN is configured. With no DSN,
//! [`init`] returns `None` **without** initialising Sentry — no client is
//! bound, no transport is spawned, and the `sentry-tracing` layer silently
//! drops every event. This satisfies the acceptance criterion "DSN unset → no
//! init, no overhead, no error".
//!
//! # Secret scrubbing
//!
//! [`scrub_event`] (see the [`scrub`] submodule) removes sensitive request
//! headers (an explicit list plus any name that looks like a credential),
//! redacts any `extra` / `contexts` / breadcrumb-`data` value whose key looks
//! like a credential (`password`, `secret`, `token`, `api_key`, `apikey`,
//! `app_key`), and redacts credentials in the request body (JSON, walked
//! recursively, or form-encoded), query string and cookie jar. It is installed
//! unconditionally as the client's `before_send` callback, so every captured
//! event is filtered.

use rustasea_config::ConfigLoader;

use crate::error::{LoggingError, Result};

mod scrub;

/// Re-export of the Sentry client guard so callers can hold it without naming
/// the `sentry` crate directly.
pub use sentry::ClientInitGuard;

/// The `before_send` secret-scrubbing hook (see the [`scrub`] submodule).
pub use scrub::scrub_event;

/// Typed `[logging.sentry]` configuration plus its `SENTRY_*` environment
/// overrides.
///
/// The struct is always present (even when the feature has no DSN), so callers
/// can branch on [`SentryConfig::enabled`] without threading an `Option`.
#[derive(Debug, Clone, PartialEq)]
pub struct SentryConfig {
    /// Sentry DSN. `None` (or a blank value) disables the integration.
    pub dsn: Option<String>,
    /// Fraction of transactions sampled for performance tracing (`0.0..=1.0`).
    pub traces_sample_rate: f32,
    /// Deployment environment label (`production`, `staging`, …).
    pub environment: Option<String>,
    /// Whether a usable DSN is configured.
    pub enabled: bool,
}

impl Default for SentryConfig {
    /// A disabled configuration: no DSN, no sampling, no environment.
    fn default() -> Self {
        Self {
            dsn: None,
            traces_sample_rate: 0.0,
            environment: None,
            enabled: false,
        }
    }
}

/// Deserialized shape of the `[logging.sentry]` table.
///
/// Kept private so the public [`SentryConfig`] can carry the derived `enabled`
/// flag without exposing a redundant field to the config file.
#[derive(Debug, Default, serde::Deserialize)]
struct SentryTable {
    /// Sentry DSN from the config file.
    #[serde(default)]
    dsn: Option<String>,
    /// Sampling rate from the config file.
    #[serde(default)]
    traces_sample_rate: Option<f32>,
    /// Environment label from the config file.
    #[serde(default)]
    environment: Option<String>,
}

impl SentryConfig {
    /// Load `[logging.sentry]` from `loader`, then apply `SENTRY_*` overrides.
    ///
    /// A missing `[logging.sentry]` table is tolerated (a disabled config); a
    /// malformed one surfaces [`LoggingError::InvalidConfig`]. The environment
    /// always wins over the file:
    ///
    /// * `SENTRY_DSN` — the DSN (blank is treated as absent).
    /// * `SENTRY_TRACES_SAMPLE_RATE` — parsed as `f32`; an invalid value is
    ///   ignored (the file value, or `0.0`, is kept).
    /// * `SENTRY_ENVIRONMENT` — the environment label.
    ///
    /// # Errors
    ///
    /// [`LoggingError::InvalidConfig`] when `[logging.sentry]` exists but cannot
    /// be deserialized.
    pub fn from_loader(loader: &ConfigLoader) -> Result<Self> {
        let table = match loader.get_key::<SentryTable>("logging.sentry") {
            Ok(table) => table,
            Err(error) => {
                if loader.inner().get_table("logging.sentry").is_err() {
                    SentryTable::default()
                } else {
                    return Err(LoggingError::InvalidConfig(error.to_string()));
                }
            }
        };

        let mut config = Self {
            dsn: table.dsn,
            traces_sample_rate: table.traces_sample_rate.unwrap_or(0.0),
            environment: table.environment,
            enabled: false,
        };
        config.apply_env();
        Ok(config)
    }

    /// Apply the `SENTRY_*` environment overrides in place.
    ///
    /// Empty values are ignored. An unparseable sampling rate is skipped so a
    /// typo never aborts boot. The derived [`SentryConfig::enabled`] flag is
    /// recomputed at the end.
    pub fn apply_env(&mut self) {
        if let Some(dsn) = env_non_empty("SENTRY_DSN") {
            self.dsn = Some(dsn);
        }
        if let Some(rate) =
            env_non_empty("SENTRY_TRACES_SAMPLE_RATE").and_then(|value| value.parse::<f32>().ok())
        {
            self.traces_sample_rate = rate;
        }
        if let Some(environment) = env_non_empty("SENTRY_ENVIRONMENT") {
            self.environment = Some(environment);
        }
        self.enabled = self
            .dsn
            .as_deref()
            .is_some_and(|dsn| !dsn.trim().is_empty());
    }
}

/// Initialise the process-global Sentry client from `config`.
///
/// Returns `None` when the integration is disabled (no DSN) or the DSN cannot be
/// parsed — in both cases **nothing** is initialised, so there is no client, no
/// transport and no overhead. On success the returned [`sentry::ClientInitGuard`]
/// **must be kept alive** for the process lifetime: dropping it flushes the send
/// queue and shuts the transport down.
///
/// The DSN is parsed eagerly with [`str::parse`] instead of
/// [`sentry::ClientOptions::dsn`] so an invalid value is a graceful `None`
/// rather than a panic.
pub fn init(config: &SentryConfig) -> Option<sentry::ClientInitGuard> {
    if !config.enabled {
        return None;
    }
    let dsn = match config.dsn.as_deref().map(str::parse::<sentry::types::Dsn>) {
        Some(Ok(dsn)) => dsn,
        Some(Err(error)) => {
            tracing::warn!(%error, "sentry: invalid DSN, error tracking disabled");
            return None;
        }
        None => return None,
    };

    // Clamp before handing the rate to the builder, which panics outside
    // `0.0..=1.0`; a misconfigured rate must not take the process down.
    let rate = config.traces_sample_rate.clamp(0.0, 1.0);

    let mut options = sentry::ClientOptions::new()
        .traces_sample_rate(rate)
        .before_send(scrub_event)
        // `apply_defaults` also installs a panic integration when the `panic`
        // feature is on; adding ours is explicit and harmless (the panic hook
        // is installed once, guarded by `Once` inside sentry-panic).
        .add_integration(sentry_panic::PanicIntegration::new());
    options.dsn = Some(dsn);
    if let Some(environment) = &config.environment {
        options.environment = Some(environment.clone().into());
    }

    Some(sentry::init(options))
}

/// The `sentry-tracing` layer that forwards `tracing` events to Sentry.
///
/// The layer is a no-op while no client is bound, so [`crate::init`] can compose
/// it unconditionally under the `sentry` feature. Events at `error` level are
/// captured as Sentry events; lower levels become breadcrumbs.
pub fn tracing_layer() -> sentry_tracing::SentryLayer<tracing_subscriber::Registry> {
    sentry_tracing::layer()
}

/// Read an environment variable, treating unset or blank values as absent.
fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_config_does_not_init() {
        let config = SentryConfig::default();
        assert!(!config.enabled);
        assert!(
            init(&config).is_none(),
            "disabled config must not initialise"
        );
    }

    #[test]
    fn blank_dsn_is_disabled() {
        let mut config = SentryConfig {
            dsn: Some("   ".to_string()),
            ..SentryConfig::default()
        };
        config.apply_env();
        assert!(!config.enabled);
    }
}
