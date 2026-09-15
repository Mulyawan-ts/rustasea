//! RustaSea Logging — Laravel-style `config/logging.toml` channels on `tracing`.
//!
//! This crate is the logging facade for RustaSea. It parses a Laravel 13.x
//! `logging.php`-shaped `[logging]` table from [`rustasea_config::ConfigLoader`]
//! and installs a single global [`tracing`] subscriber that fans events out to
//! the configured channels.
//!
//! # Channels
//!
//! Every Laravel channel name is recognised and parsed. Drivers map onto
//! `tracing` as follows:
//!
//! | Driver | Behaviour |
//! | --- | --- |
//! | `single` | Non-blocking file appender (no rotation). |
//! | `daily` | Rolling file appender, one file per day, `max_files` pruning. |
//! | `monthly` | Rolling file appender (see the rotation note below). |
//! | `stack` | Composes the referenced channels into one subscriber. |
//! | `stderr` | Formatted output to standard error. |
//! | `stdout` | Formatted output to standard output. |
//! | `errorlog` | Documented fallback: formatted output to standard error. |
//! | `null` | No-op channel — events are dropped. |
//! | `emergency` | Non-blocking file appender at the configured `path`. |
//! | `slack` / `papertrail` / `syslog` | Recognised but **unsupported**: selecting one returns [`LoggingError::UnsupportedDriver`]. |
//!
//! ## Rotation note
//!
//! `tracing-appender` exposes minutely/hourly/daily/weekly rotation but has no
//! monthly interval. The `monthly` driver therefore uses the closest supported
//! interval (daily) and honours `max_files` (default [`DEFAULT_MONTHLY_FILES`]).
//! This is a documented approximation, not a silent downgrade.
//!
//! # Environment overrides
//!
//! [`init`] honours `LOG_CHANNEL` (default channel), `LOG_LEVEL` (fallback
//! level for channels without an explicit one), `LOG_STACK` (comma-separated
//! stack members) and `LOG_DAILY_DAYS` (daily retention). `LOG_SLACK_*` values
//! are accepted for parity but are inert because the `slack` driver is
//! unsupported.
//!
//! # Example
//!
//! ```no_run
//! use rustasea_logging::init_from_config;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let loader = rustasea_config::ConfigLoader::load()?;
//! let _guard = init_from_config(&loader)?;
//! tracing::info!("application started");
//! # Ok(())
//! # }
//! ```

pub mod config;
pub mod error;
pub mod init;
/// Sentry error-tracking integration (ADOPT-004) — opt-in via the `sentry`
/// feature, inert until a DSN is configured.
#[cfg(feature = "sentry")]
pub mod sentry;

pub use config::{
    ChannelConfig, DeprecationsConfig, Driver, LoggingConfig, DEFAULT_CHANNEL, DEFAULT_DAILY_FILES,
    DEFAULT_MONTHLY_FILES,
};
pub use error::{LoggingError, Result};
pub use init::{build, init, init_from_config, LoggingGuard};

/// Sentry re-exports (ADOPT-004): config parsing, client init, and the
/// `before_send` secret-scrubbing hook.
#[cfg(feature = "sentry")]
pub use sentry::{init as init_sentry, scrub_event, ClientInitGuard, SentryConfig};
