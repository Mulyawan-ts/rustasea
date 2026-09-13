//! Typed `[mail]` configuration — Laravel 13.x `mail.php` parity.
//!
//! [`MailConfig`] mirrors the shape of Laravel's `config/mail.php`: a `default`
//! mailer selector, a `from` block, and a map of named `[mail.mailers.*]`
//! entries. Every Laravel mailer name is represented; each entry carries the
//! union of the fields the individual transports need.
//!
//! The table is deserialized from [`rustasea_config::ConfigLoader`] via
//! [`MailConfig::from_loader`], which also applies the `MAIL_*` environment
//! overrides. A missing `[mail]` table is tolerated (an empty config selecting
//! the [`DEFAULT_MAILER`]), while a malformed one is a typed
//! [`MailConfigError::InvalidConfig`].
//!
//! [`mailer_from_config`] turns a [`MailConfig`] into a ready
//! [`Arc<dyn Mailer>`](crate::Mailer), wrapping the transport in a
//! [`FromMailer`](crate::mailer::FromMailer) when `[mail.from]` is set.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Deserialize;

use rustasea_config::ConfigLoader;

use crate::address::MailAddress;
use crate::error::MailConfigError;
use crate::mailer::{ArrayMailer, FailoverMailer, FromMailer, LogMailer, Mailer};

/// Mailer selected when `[mail].default` is absent.
pub const DEFAULT_MAILER: &str = "log";

/// Transport name for the `log` mailer.
pub const TRANSPORT_LOG: &str = "log";
/// Transport name for the in-memory `array` mailer.
pub const TRANSPORT_ARRAY: &str = "array";
/// Transport name for the SMTP mailer (requires the crate's `smtp` feature).
pub const TRANSPORT_SMTP: &str = "smtp";
/// Transport name for the failover mailer.
pub const TRANSPORT_FAILOVER: &str = "failover";

/// Transports recognised for Laravel parity but not yet implemented.
///
/// Selecting one is a typed [`MailConfigError::UnsupportedTransport`] rather
/// than a panic or a silent downgrade.
pub const UNSUPPORTED_TRANSPORTS: [&str; 4] = ["ses", "postmark", "resend", "sendmail"];

/// The sender applied to messages that do not set their own `from`
/// (`[mail.from]`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct FromConfig {
    /// Bare email address.
    #[serde(default)]
    pub address: String,
    /// Optional display name; falls back to the application name.
    #[serde(default)]
    pub name: Option<String>,
}

impl FromConfig {
    /// True when no sender address is configured.
    pub fn is_empty(&self) -> bool {
        self.address.trim().is_empty()
    }

    /// Build the typed [`MailAddress`], or `None` when the address is blank.
    pub fn to_address(&self) -> Option<MailAddress> {
        if self.is_empty() {
            return None;
        }
        Some(MailAddress::new(
            self.address.trim(),
            self.name.clone().filter(|name| !name.trim().is_empty()),
        ))
    }
}

/// One named mailer.
///
/// The struct is the union of every Laravel mailer shape, so a mailer simply
/// leaves the fields its transport does not use unset. `transport` defaults to
/// the empty string and is resolved from the entry key when absent (see
/// [`MailerConfig::resolve_transport`]).
#[derive(Debug, Clone, Deserialize)]
pub struct MailerConfig {
    /// Transport selector (`smtp`, `log`, `array`, `failover`, or one of the
    /// recognised-but-unsupported names).
    #[serde(default)]
    pub transport: String,
    /// Connection scheme (`smtp` or `smtps`; parsed for parity).
    #[serde(default)]
    pub scheme: Option<String>,
    /// Pre-built DSN (parsed for parity; currently inert).
    #[serde(default)]
    pub url: Option<String>,
    /// SMTP relay host.
    #[serde(default)]
    pub host: Option<String>,
    /// SMTP relay port.
    #[serde(default)]
    pub port: Option<u16>,
    /// SMTP username.
    #[serde(default)]
    pub username: Option<String>,
    /// SMTP password.
    #[serde(default)]
    pub password: Option<String>,
    /// Socket timeout in seconds (parsed for parity; inert).
    #[serde(default)]
    pub timeout: Option<u64>,
    /// EHLO domain presented to the relay (parsed for parity; inert).
    #[serde(default)]
    pub local_domain: Option<String>,
    /// Log channel name (parsed for parity; inert).
    #[serde(default)]
    pub channel: Option<String>,
    /// `sendmail` binary path (recognised; transport unsupported).
    #[serde(default)]
    pub path: Option<String>,
    /// Member mailer names (`failover`).
    #[serde(default)]
    pub mailers: Vec<String>,
    /// Retry delay in seconds (`failover`; parsed for parity, inert).
    #[serde(default)]
    pub retry_after: Option<u64>,
}

impl MailerConfig {
    /// Create a mailer for `transport` with every optional field unset.
    pub fn new(transport: impl Into<String>) -> Self {
        Self {
            transport: transport.into(),
            scheme: None,
            url: None,
            host: None,
            port: None,
            username: None,
            password: None,
            timeout: None,
            local_domain: None,
            channel: None,
            path: None,
            mailers: Vec::new(),
            retry_after: None,
        }
    }

    /// Set the SMTP relay host (builder style).
    #[must_use]
    pub fn with_host(mut self, host: impl Into<String>) -> Self {
        self.host = Some(host.into());
        self
    }

    /// Set the SMTP relay port (builder style).
    #[must_use]
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    /// Set the member mailers of a failover (builder style).
    #[must_use]
    pub fn with_mailers<I, S>(mut self, mailers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.mailers = mailers.into_iter().map(Into::into).collect();
        self
    }

    /// Resolve the transport name, falling back to `key` (the table name) when
    /// the entry declares no explicit `transport`.
    pub fn resolve_transport(&self, key: &str) -> String {
        let declared = self.transport.trim();
        if declared.is_empty() {
            key.trim().to_ascii_lowercase()
        } else {
            declared.to_ascii_lowercase()
        }
    }
}

/// Typed `[mail]` table.
#[derive(Debug, Clone, Deserialize)]
pub struct MailConfig {
    /// Name of the mailer used when none is requested.
    #[serde(default = "default_mailer_name")]
    pub default: String,
    /// Sender applied to messages without their own `from`.
    #[serde(default)]
    pub from: FromConfig,
    /// Named mailers, ordered by name for deterministic iteration.
    #[serde(default)]
    pub mailers: BTreeMap<String, MailerConfig>,
}

impl Default for MailConfig {
    /// An empty config selecting the [`DEFAULT_MAILER`].
    fn default() -> Self {
        Self {
            default: DEFAULT_MAILER.to_string(),
            from: FromConfig::default(),
            mailers: BTreeMap::new(),
        }
    }
}

impl MailConfig {
    /// Create a config selecting `default` as the active mailer.
    pub fn new(default: impl Into<String>) -> Self {
        Self {
            default: default.into(),
            ..Self::default()
        }
    }

    /// Register a mailer (builder style).
    #[must_use]
    pub fn with_mailer(mut self, name: impl Into<String>, mailer: MailerConfig) -> Self {
        self.mailers.insert(name.into(), mailer);
        self
    }

    /// Set the default sender (builder style).
    #[must_use]
    pub fn with_from(mut self, from: FromConfig) -> Self {
        self.from = from;
        self
    }

    /// Deserialize `[mail]` from a layered [`ConfigLoader`].
    ///
    /// A missing `[mail]` table yields [`MailConfig::default`] (tolerated,
    /// matching the loader's missing-file policy); a malformed table surfaces
    /// [`MailConfigError::InvalidConfig`]. The `MAIL_*` environment overrides
    /// are applied afterwards, so they always win over the file.
    ///
    /// # Errors
    ///
    /// [`MailConfigError::InvalidConfig`] when the `[mail]` table exists but
    /// cannot be deserialized.
    pub fn from_loader(loader: &ConfigLoader) -> Result<Self, MailConfigError> {
        let mut config = match loader.get_key::<MailConfig>("mail") {
            Ok(config) => config,
            Err(error) => {
                if loader.inner().get_table("mail").is_err() {
                    MailConfig::default()
                } else {
                    return Err(MailConfigError::InvalidConfig(error.to_string()));
                }
            }
        };
        config.apply_env();
        Ok(config)
    }

    /// Apply the `MAIL_*` environment overrides (env > file).
    ///
    /// `MAIL_MAILER` replaces the default selector, `MAIL_HOST`/`MAIL_PORT`/
    /// `MAIL_USERNAME`/`MAIL_PASSWORD` replace the corresponding
    /// `[mail.mailers.smtp]` fields, and `MAIL_FROM_ADDRESS`/`MAIL_FROM_NAME`
    /// replace the `[mail.from]` block. Empty values are ignored.
    pub fn apply_env(&mut self) {
        if let Some(mailer) = env_non_empty("MAIL_MAILER") {
            self.default = mailer;
        }
        if let Some(address) = env_non_empty("MAIL_FROM_ADDRESS") {
            self.from.address = address;
        }
        if let Some(name) = env_non_empty("MAIL_FROM_NAME") {
            self.from.name = Some(name);
        }

        let host = env_non_empty("MAIL_HOST");
        let port = env_non_empty("MAIL_PORT").and_then(|value| value.parse::<u16>().ok());
        let username = env_non_empty("MAIL_USERNAME");
        let password = env_non_empty("MAIL_PASSWORD");
        if host.is_none() && port.is_none() && username.is_none() && password.is_none() {
            return;
        }
        let smtp = self
            .mailers
            .entry(TRANSPORT_SMTP.to_string())
            .or_insert_with(|| MailerConfig::new(TRANSPORT_SMTP));
        if let Some(host) = host {
            smtp.host = Some(host);
        }
        if let Some(port) = port {
            smtp.port = Some(port);
        }
        if let Some(username) = username {
            smtp.username = Some(username);
        }
        if let Some(password) = password {
            smtp.password = Some(password);
        }
    }

    /// Look up a mailer by name.
    ///
    /// # Errors
    ///
    /// [`MailConfigError::UnknownDefaultMailer`] when no such mailer is
    /// defined (the name is reported in the message).
    pub fn mailer(&self, name: &str) -> Result<&MailerConfig, MailConfigError> {
        self.mailers
            .get(name)
            .ok_or_else(|| MailConfigError::UnknownDefaultMailer(name.to_string()))
    }

    /// Look up the default mailer.
    ///
    /// # Errors
    ///
    /// [`MailConfigError::UnknownDefaultMailer`] when the default selector
    /// names a mailer that is not defined.
    pub fn default_mailer(&self) -> Result<&MailerConfig, MailConfigError> {
        self.mailer(&self.default)
    }

    /// The configured sender address, when `[mail.from].address` is non-empty.
    pub fn from_address(&self) -> Option<MailAddress> {
        self.from.to_address()
    }
}

/// Build a ready [`Mailer`] from a [`MailConfig`].
///
/// The default mailer is resolved from `config.default` and dispatched to the
/// transport it names:
///
/// | Transport | Mailer |
/// | --- | --- |
/// | `log` | [`LogMailer`] |
/// | `array` | [`ArrayMailer`] |
/// | `smtp` | `SmtpMailer` (requires the `smtp` feature) |
/// | `failover` | [`FailoverMailer`] over its member mailers |
///
/// When `[mail.from]` is set, the transport is wrapped in a
/// [`FromMailer`](crate::mailer::FromMailer) so messages without their own
/// sender inherit the configured address.
///
/// # Errors
///
/// - [`MailConfigError::UnknownDefaultMailer`] when `default` is undefined;
/// - [`MailConfigError::UnknownTransport`] for an unrecognised transport;
/// - [`MailConfigError::UnsupportedTransport`] for a recognised-but-unbuilt
///   transport (`ses`, `postmark`, `resend`, `sendmail`);
/// - [`MailConfigError::MissingSmtpHost`] when `smtp` has no host;
/// - [`MailConfigError::MissingSmtpFeature`] when `smtp` is selected but the
///   crate was built without the `smtp` feature;
/// - [`MailConfigError::EmptyFailover`] when a `failover` lists no members.
pub fn mailer_from_config(config: &MailConfig) -> Result<Arc<dyn Mailer>, MailConfigError> {
    let name = config.default.clone();
    let transport = build_named(config, &name)?;
    Ok(apply_from(transport, config))
}

/// Wrap `transport` in a [`FromMailer`] when `config` carries a sender.
fn apply_from(transport: Arc<dyn Mailer>, config: &MailConfig) -> Arc<dyn Mailer> {
    match config.from_address() {
        Some(from) => Arc::new(FromMailer::new(transport, from)),
        None => transport,
    }
}

/// Build the mailer registered under `name`.
fn build_named(config: &MailConfig, name: &str) -> Result<Arc<dyn Mailer>, MailConfigError> {
    let mailer = config.mailer(name)?;
    build_mailer(config, name, mailer)
}

/// Build a single [`MailerConfig`], recursing into `failover` members.
fn build_mailer(
    config: &MailConfig,
    key: &str,
    mailer: &MailerConfig,
) -> Result<Arc<dyn Mailer>, MailConfigError> {
    let transport = mailer.resolve_transport(key);
    match transport.as_str() {
        TRANSPORT_LOG => Ok(Arc::new(LogMailer::new())),
        TRANSPORT_ARRAY => Ok(Arc::new(ArrayMailer::new())),
        TRANSPORT_SMTP => build_smtp(key, mailer),
        TRANSPORT_FAILOVER => build_failover(config, key, mailer),
        other if UNSUPPORTED_TRANSPORTS.contains(&other) => {
            Err(MailConfigError::UnsupportedTransport(other.to_string()))
        }
        other => Err(MailConfigError::UnknownTransport(other.to_string())),
    }
}

/// Validate the SMTP host (a config concern, checked regardless of feature)
/// and then build the transport, which is gated behind the `smtp` feature.
fn build_smtp(key: &str, mailer: &MailerConfig) -> Result<Arc<dyn Mailer>, MailConfigError> {
    let host = mailer
        .host
        .as_deref()
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .ok_or_else(|| MailConfigError::MissingSmtpHost(key.to_string()))?;
    build_smtp_transport(mailer, host)
}

/// Build the SMTP transport with the `smtp` feature enabled.
#[cfg(feature = "smtp")]
fn build_smtp_transport(
    mailer: &MailerConfig,
    host: &str,
) -> Result<Arc<dyn Mailer>, MailConfigError> {
    let port = mailer.port.unwrap_or(25);
    let credentials = match (mailer.username.as_deref(), mailer.password.as_deref()) {
        (Some(username), password) if !username.trim().is_empty() => {
            Some((username.to_string(), password.unwrap_or("").to_string()))
        }
        _ => None,
    };
    let timeout = mailer
        .timeout
        .filter(|timeout| *timeout > 0)
        .map(std::time::Duration::from_secs);
    let smtp = crate::smtp::SmtpMailer::relay_with_timeout(host, port, credentials, timeout)
        .map_err(|error| MailConfigError::InvalidConfig(error.to_string()))?;
    Ok(Arc::new(smtp))
}

/// Build the SMTP transport without the `smtp` feature: always a typed error.
#[cfg(not(feature = "smtp"))]
fn build_smtp_transport(
    _mailer: &MailerConfig,
    _host: &str,
) -> Result<Arc<dyn Mailer>, MailConfigError> {
    Err(MailConfigError::MissingSmtpFeature)
}

/// Build a `failover` mailer over its member mailers, tried in order.
fn build_failover(
    config: &MailConfig,
    key: &str,
    mailer: &MailerConfig,
) -> Result<Arc<dyn Mailer>, MailConfigError> {
    if mailer.mailers.is_empty() {
        return Err(MailConfigError::EmptyFailover(key.to_string()));
    }
    let mut members = Vec::with_capacity(mailer.mailers.len());
    for member in &mailer.mailers {
        members.push(build_named(config, member)?);
    }
    Ok(Arc::new(FailoverMailer::new(members)))
}

/// Serde default for [`MailConfig::default`].
fn default_mailer_name() -> String {
    DEFAULT_MAILER.to_string()
}

/// Read an environment variable, treating unset or blank values as absent.
fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
}
