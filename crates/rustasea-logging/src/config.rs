//! Typed `[logging]` configuration — Laravel 13.x `logging.php` parity.
//!
//! [`LoggingConfig`] mirrors the shape of Laravel's `config/logging.php`:
//! a `default` channel selector, a `deprecations` block, and a `channels` map
//! of named channels. Every Laravel channel name is represented; each channel
//! carries the union of the fields the individual drivers need.
//!
//! The table is deserialized from [`rustasea_config::ConfigLoader`] via
//! [`LoggingConfig::from_loader`], which also applies the `LOG_*` environment
//! overrides. A missing `[logging]` table is tolerated (an empty config with
//! the [`DEFAULT_CHANNEL`] selector), while a malformed one is a typed
//! [`LoggingError::InvalidConfig`].

use std::collections::BTreeMap;

use serde::Deserialize;

use rustasea_config::ConfigLoader;

use crate::error::{LoggingError, Result};

/// Channel selected when `[logging].default` is absent.
pub const DEFAULT_CHANNEL: &str = "stack";

/// Level used when neither the channel nor `LOG_LEVEL` names one.
pub const DEFAULT_LEVEL: &str = "debug";

/// Path used by file channels that omit an explicit `path`.
pub const DEFAULT_LOG_PATH: &str = "storage/logs/rustasea.log";

/// Default retention for the `daily` driver (matches Laravel's `LOG_DAILY_DAYS`).
pub const DEFAULT_DAILY_FILES: usize = 14;

/// Default retention for the `monthly` driver (matches Laravel's `3`).
pub const DEFAULT_MONTHLY_FILES: usize = 3;

/// Deprecation-reporting configuration (`[logging.deprecations]`).
///
/// Parsed for Laravel parity. RustaSea does not yet route `tracing`
/// deprecations through a dedicated channel, so these values are inert but
/// preserved so a migrated config round-trips unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct DeprecationsConfig {
    /// Channel that would receive deprecation notices (`null` disables them).
    #[serde(default)]
    pub channel: Option<String>,
    /// Whether a stack trace would be attached to each notice.
    #[serde(default)]
    pub trace: bool,
}

/// One named logging channel.
///
/// The struct is the union of every Laravel channel shape, so a channel simply
/// leaves the fields its driver does not use unset. `driver` defaults to the
/// empty string to accommodate Laravel's `emergency` channel, which declares
/// only a `path`; see [`ChannelConfig::resolve_driver`].
#[derive(Debug, Clone, Deserialize)]
pub struct ChannelConfig {
    /// Driver selector (`single`, `daily`, `monthly`, `stack`, `stderr`,
    /// `stdout`, `null`, `emergency`, `errorlog`, `slack`, `papertrail`,
    /// `syslog`, or `monolog`).
    #[serde(default)]
    pub driver: String,
    /// Minimum level emitted by this channel (falls back to `LOG_LEVEL`).
    #[serde(default)]
    pub level: Option<String>,
    /// Log file path (file channels).
    #[serde(default)]
    pub path: Option<String>,
    /// Retention for rolling channels; accepts the Laravel `days` alias.
    #[serde(default, alias = "days")]
    pub max_files: Option<usize>,
    /// Webhook URL (`slack`; recognised but unsupported).
    #[serde(default)]
    pub url: Option<String>,
    /// Bot username (`slack`; recognised but unsupported).
    #[serde(default)]
    pub username: Option<String>,
    /// Bot emoji (`slack`; recognised but unsupported).
    #[serde(default)]
    pub emoji: Option<String>,
    /// Member channel names (`stack`).
    #[serde(default)]
    pub channels: Vec<String>,
    /// Whether a failing member is ignored (`stack`); parsed for parity.
    #[serde(default)]
    pub ignore_exceptions: bool,
    /// Whether message placeholders are substituted; parsed for parity.
    #[serde(default)]
    pub replace_placeholders: bool,
    /// Laravel `monolog` handler class; used to resolve a `monolog` driver.
    #[serde(default)]
    pub handler: Option<String>,
    /// Syslog facility (`syslog`; recognised but unsupported).
    #[serde(default)]
    pub facility: Option<String>,
    /// Whether traces are captured (parsed for parity).
    #[serde(default)]
    pub trace: bool,
}

impl ChannelConfig {
    /// Create a channel for `driver` with every optional field unset.
    pub fn new(driver: impl Into<String>) -> Self {
        Self {
            driver: driver.into(),
            level: None,
            path: None,
            max_files: None,
            url: None,
            username: None,
            emoji: None,
            channels: Vec::new(),
            ignore_exceptions: false,
            replace_placeholders: false,
            handler: None,
            facility: None,
            trace: false,
        }
    }

    /// Set the channel's minimum level (builder style).
    #[must_use]
    pub fn with_level(mut self, level: impl Into<String>) -> Self {
        self.level = Some(level.into());
        self
    }

    /// Set the channel's log file path (builder style).
    #[must_use]
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Set the channel's retention for rolling drivers (builder style).
    #[must_use]
    pub fn with_max_files(mut self, max_files: usize) -> Self {
        self.max_files = Some(max_files);
        self
    }

    /// Set the channel's stack members (builder style).
    #[must_use]
    pub fn with_channels<I, S>(mut self, channels: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.channels = channels.into_iter().map(Into::into).collect();
        self
    }

    /// Resolve the typed [`Driver`] for this channel.
    ///
    /// An empty `driver` is treated as [`Driver::Emergency`] when a `path` is
    /// present (Laravel's `emergency` shape). A `driver = "monolog"` is mapped
    /// through the `handler` field: `NullHandler` → [`Driver::Null`],
    /// `StreamHandler` → [`Driver::Stderr`]; any other handler is unsupported.
    ///
    /// # Errors
    ///
    /// [`LoggingError::UnknownDriver`] for an unrecognised driver, or
    /// [`LoggingError::UnsupportedDriver`] for a `monolog` handler this crate
    /// cannot map.
    pub fn resolve_driver(&self) -> Result<Driver> {
        let name = self.driver.trim();
        if name.is_empty() {
            return if self.path.is_some() {
                Ok(Driver::Emergency)
            } else {
                Err(LoggingError::UnknownDriver(String::new()))
            };
        }
        if name.eq_ignore_ascii_case("monolog") {
            return self.resolve_monolog_driver();
        }
        Driver::from_name(name)
    }

    /// Map a Laravel `monolog` channel onto a supported driver via its handler.
    fn resolve_monolog_driver(&self) -> Result<Driver> {
        let handler = self.handler.as_deref().unwrap_or("").to_ascii_lowercase();
        if handler.contains("nullhandler") {
            Ok(Driver::Null)
        } else if handler.contains("streamhandler") {
            Ok(Driver::Stderr)
        } else {
            Err(LoggingError::UnsupportedDriver("monolog".to_string()))
        }
    }
}

/// The driver backing a channel.
///
/// Variants exist for every Laravel channel; [`Driver::is_supported`] reports
/// which ones this crate can actually install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Driver {
    /// Non-blocking file appender without rotation.
    Single,
    /// Rolling file appender, one file per day.
    Daily,
    /// Rolling file appender approximating Laravel's monthly rotation.
    Monthly,
    /// Composes the referenced channels.
    Stack,
    /// Formatted output to standard error.
    Stderr,
    /// Formatted output to standard output.
    Stdout,
    /// Drops every event.
    Null,
    /// Non-blocking file appender for emergency logging.
    Emergency,
    /// Documented fallback: formatted output to standard error.
    Errorlog,
    /// Slack webhook — recognised but unsupported.
    Slack,
    /// Papertrail — recognised but unsupported.
    Papertrail,
    /// Syslog — recognised but unsupported.
    Syslog,
}

impl Driver {
    /// Parse a driver name (case-insensitive).
    ///
    /// # Errors
    ///
    /// [`LoggingError::UnknownDriver`] when `name` matches no known driver.
    pub fn from_name(name: &str) -> Result<Self> {
        let driver = match name.to_ascii_lowercase().as_str() {
            "single" => Driver::Single,
            "daily" => Driver::Daily,
            "monthly" => Driver::Monthly,
            "stack" => Driver::Stack,
            "stderr" => Driver::Stderr,
            "stdout" => Driver::Stdout,
            "null" => Driver::Null,
            "emergency" => Driver::Emergency,
            "errorlog" => Driver::Errorlog,
            "slack" => Driver::Slack,
            "papertrail" => Driver::Papertrail,
            "syslog" => Driver::Syslog,
            _ => return Err(LoggingError::UnknownDriver(name.to_string())),
        };
        Ok(driver)
    }

    /// Canonical driver name.
    pub fn name(self) -> &'static str {
        match self {
            Driver::Single => "single",
            Driver::Daily => "daily",
            Driver::Monthly => "monthly",
            Driver::Stack => "stack",
            Driver::Stderr => "stderr",
            Driver::Stdout => "stdout",
            Driver::Null => "null",
            Driver::Emergency => "emergency",
            Driver::Errorlog => "errorlog",
            Driver::Slack => "slack",
            Driver::Papertrail => "papertrail",
            Driver::Syslog => "syslog",
        }
    }

    /// True when this crate can install the driver.
    ///
    /// `slack`, `papertrail` and `syslog` are recognised for config parity but
    /// are not implemented; selecting one yields
    /// [`LoggingError::UnsupportedDriver`].
    pub fn is_supported(self) -> bool {
        !matches!(self, Driver::Slack | Driver::Papertrail | Driver::Syslog)
    }
}

/// Typed `[logging]` table.
#[derive(Debug, Clone, Deserialize)]
pub struct LoggingConfig {
    /// Name of the channel used when none is requested.
    #[serde(default = "default_channel_name")]
    pub default: String,
    /// Deprecation-reporting settings (parsed for parity).
    #[serde(default)]
    pub deprecations: DeprecationsConfig,
    /// Named channels, ordered by name for deterministic iteration.
    #[serde(default)]
    pub channels: BTreeMap<String, ChannelConfig>,
    /// Process-level fallback level from `LOG_LEVEL` (never serialized).
    #[serde(skip)]
    pub level: Option<String>,
}

impl Default for LoggingConfig {
    /// An empty config selecting the [`DEFAULT_CHANNEL`].
    fn default() -> Self {
        Self {
            default: DEFAULT_CHANNEL.to_string(),
            deprecations: DeprecationsConfig::default(),
            channels: BTreeMap::new(),
            level: None,
        }
    }
}

impl LoggingConfig {
    /// Create a config with `default` as the selected channel.
    pub fn new(default: impl Into<String>) -> Self {
        Self {
            default: default.into(),
            ..Self::default()
        }
    }

    /// Register a channel (builder style).
    #[must_use]
    pub fn with_channel(mut self, name: impl Into<String>, channel: ChannelConfig) -> Self {
        self.channels.insert(name.into(), channel);
        self
    }

    /// Deserialize `[logging]` from a layered [`ConfigLoader`].
    ///
    /// A missing `[logging]` table yields [`LoggingConfig::default`] (tolerated,
    /// matching the loader's missing-file policy); a malformed table surfaces
    /// [`LoggingError::InvalidConfig`]. `LOG_CHANNEL`, `LOG_STACK`,
    /// `LOG_DAILY_DAYS` and `LOG_LEVEL` are applied afterwards.
    ///
    /// # Errors
    ///
    /// [`LoggingError::InvalidConfig`] when the `[logging]` table exists but
    /// cannot be deserialized.
    pub fn from_loader(loader: &ConfigLoader) -> Result<Self> {
        let mut config = match loader.get_key::<LoggingConfig>("logging") {
            Ok(config) => config,
            Err(error) => {
                if loader.inner().get_table("logging").is_err() {
                    LoggingConfig::default()
                } else {
                    return Err(LoggingError::InvalidConfig(error.to_string()));
                }
            }
        };
        config.apply_env();
        Ok(config)
    }

    /// Apply the `LOG_*` environment overrides.
    ///
    /// `LOG_CHANNEL` replaces the default selector, `LOG_STACK` replaces the
    /// `stack` channel's members, `LOG_DAILY_DAYS` replaces the `daily`
    /// channel's retention, and `LOG_LEVEL` becomes the process-level fallback
    /// level. Empty values are ignored.
    pub fn apply_env(&mut self) {
        if let Some(channel) = env_non_empty("LOG_CHANNEL") {
            self.default = channel;
        }
        if let Some(stack) = env_non_empty("LOG_STACK") {
            let members: Vec<String> = stack
                .split(',')
                .map(str::trim)
                .filter(|member| !member.is_empty())
                .map(str::to_string)
                .collect();
            if let Some(channel) = self.channels.get_mut("stack") {
                if !members.is_empty() {
                    channel.channels = members;
                }
            }
        }
        if let Some(days) = env_non_empty("LOG_DAILY_DAYS").and_then(|value| value.parse().ok()) {
            if let Some(channel) = self.channels.get_mut("daily") {
                channel.max_files = Some(days);
            }
        }
        if let Some(level) = env_non_empty("LOG_LEVEL") {
            self.level = Some(level);
        }
    }

    /// Look up a channel by name.
    ///
    /// # Errors
    ///
    /// [`LoggingError::UnknownChannel`] when no such channel is defined.
    pub fn channel(&self, name: &str) -> Result<&ChannelConfig> {
        self.channels
            .get(name)
            .ok_or_else(|| LoggingError::UnknownChannel(name.to_string()))
    }

    /// Look up the default channel.
    ///
    /// # Errors
    ///
    /// [`LoggingError::UnknownChannel`] when the default selector names a
    /// channel that is not defined.
    pub fn default_channel(&self) -> Result<&ChannelConfig> {
        self.channel(&self.default)
    }

    /// Effective level for `channel`: its own `level`, else `LOG_LEVEL`, else
    /// [`DEFAULT_LEVEL`].
    pub fn resolve_level<'a>(&'a self, channel: &'a ChannelConfig) -> &'a str {
        channel
            .level
            .as_deref()
            .or(self.level.as_deref())
            .unwrap_or(DEFAULT_LEVEL)
    }
}

/// Serde default for [`LoggingConfig::default`].
fn default_channel_name() -> String {
    DEFAULT_CHANNEL.to_string()
}

/// Read an environment variable, treating unset or blank values as absent.
fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
}
