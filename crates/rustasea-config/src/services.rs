//! Typed `[services]` credentials — Laravel 13.x `services.php` parity.
//!
//! [`ServicesConfig`] mirrors the shape of Laravel's `config/services.php`: a
//! set of third-party credential blocks for Postmark, Resend, AWS SES, and
//! Slack notifications. This is a *credentials surface*: no runtime consumer
//! is wired yet, but AI/mail providers and future notification channels read
//! it through the typed accessors below.
//!
//! Build one with [`ServicesConfig::from_loader`]. The table is read from a
//! layered [`rustasea_config::ConfigLoader`] and the Laravel-style environment
//! overrides are applied afterwards (env > file):
//!
//! | Field | Environment variable |
//! | --- | --- |
//! | `[services.postmark].key` | `POSTMARK_API_KEY` |
//! | `[services.resend].key` | `RESEND_API_KEY` |
//! | `[services.ses].key` | `AWS_ACCESS_KEY_ID` |
//! | `[services.ses].secret` | `AWS_SECRET_ACCESS_KEY` |
//! | `[services.slack.notifications].bot_user_oauth_token` | `SLACK_BOT_USER_OAUTH_TOKEN` |
//! | `[services.slack.notifications].channel` | `SLACK_BOT_USER_DEFAULT_CHANNEL` |
//!
//! The loader's own overlay can also set nested values with the `__`
//! separator (for example `SERVICES__POSTMARK__KEY`), since it lowercases
//! environment keys and splits them on `__`.
//!
//! An empty string is treated as "unset" (`None`), and a missing
//! `[services.*]` block is fine (every accessor returns `None`). A malformed
//! value — for example a number where a string is expected — is a typed
//! [`ServicesConfigError::Invalid`].

use std::error::Error;
use std::fmt;

use config::{Map, Value, ValueKind};

use crate::ConfigLoader;

/// AWS region used when `[services.ses].region` is absent or blank.
///
/// Matches Laravel's `config/services.php` default.
pub const DEFAULT_SES_REGION: &str = "us-east-1";

/// Errors raised while loading `[services]` configuration.
///
/// Kept small and dedicated (the crate does not depend on `thiserror`) so a
/// caller can match on it without pulling in any other subsystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServicesConfigError {
    /// The `[services]` table exists but a value has the wrong shape (for
    /// example a number where a string is expected).
    Invalid(String),
}

impl fmt::Display for ServicesConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => write!(f, "services configuration invalid: {message}"),
        }
    }
}

impl Error for ServicesConfigError {}

/// AWS SES credentials (`[services.ses]`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SesCredentials {
    /// AWS access key id.
    pub key: String,
    /// AWS secret access key.
    pub secret: String,
    /// AWS region; [`DEFAULT_SES_REGION`] when the file leaves it blank.
    pub region: String,
}

/// Slack notification credentials (`[services.slack.notifications]`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SlackNotifications {
    /// Slack bot user OAuth token.
    pub bot_user_oauth_token: String,
    /// Default channel used for notifications.
    pub channel: String,
}

/// Typed `[services]` credentials.
///
/// Every field is optional: a blank or missing value is stored as `None`. Use
/// [`ServicesConfig::from_loader`] to build one from configuration, or
/// [`ServicesConfig::default`] for an all-`None` config.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServicesConfig {
    /// Postmark server API token.
    postmark: Option<String>,
    /// Resend API key.
    resend: Option<String>,
    /// AWS SES credentials.
    ses: Option<SesCredentials>,
    /// Slack notification credentials.
    slack: Option<SlackNotifications>,
}

impl ServicesConfig {
    /// The Postmark server API token, or `None` when unset.
    #[must_use]
    pub fn postmark_key(&self) -> Option<String> {
        self.postmark.clone()
    }

    /// The Resend API key, or `None` when unset.
    #[must_use]
    pub fn resend_key(&self) -> Option<String> {
        self.resend.clone()
    }

    /// The AWS SES credentials, or `None` when neither the key nor the secret
    /// is set.
    #[must_use]
    pub fn ses_credentials(&self) -> Option<SesCredentials> {
        self.ses.clone()
    }

    /// The Slack notification credentials, or `None` when neither the token
    /// nor the channel is set.
    #[must_use]
    pub fn slack_notifications(&self) -> Option<SlackNotifications> {
        self.slack.clone()
    }

    /// Deserialize `[services]` from a layered [`ConfigLoader`].
    ///
    /// A missing `[services]` table yields an all-`None` config (tolerated,
    /// matching the loader's missing-file policy); a malformed table surfaces
    /// [`ServicesConfigError::Invalid`]. The Laravel-style environment
    /// overrides listed at the module level are applied afterwards, so they
    /// always win over the file.
    ///
    /// # Errors
    ///
    /// [`ServicesConfigError::Invalid`] when `[services]` (or one of its
    /// blocks) exists but cannot be interpreted — for example a number where a
    /// string is expected, or a scalar where a table is expected.
    pub fn from_loader(loader: &ConfigLoader) -> Result<Self, ServicesConfigError> {
        let mut config = Self::from_services_table(loader)?;
        config.apply_env();
        config.normalize();
        Ok(config)
    }

    /// Parse the `[services]` table, tolerating an absent section.
    fn from_services_table(loader: &ConfigLoader) -> Result<Self, ServicesConfigError> {
        let services = match loader.inner().get_table("services") {
            Ok(table) => table,
            Err(error) => {
                if is_not_found(&error) {
                    return Ok(Self::default());
                }
                return Err(ServicesConfigError::Invalid(error.to_string()));
            }
        };

        Ok(Self {
            postmark: sub_table(&services, "postmark")?
                .map(|table| string_field(table, "key", "services.postmark.key"))
                .transpose()?
                .flatten(),
            resend: sub_table(&services, "resend")?
                .map(|table| string_field(table, "key", "services.resend.key"))
                .transpose()?
                .flatten(),
            ses: parse_ses(&services)?,
            slack: parse_slack(&services)?,
        })
    }

    /// Apply the Laravel-style environment overrides (env > file).
    ///
    /// Blank values are ignored. Overriding only one half of a pair (for
    /// example `AWS_ACCESS_KEY_ID` without `AWS_SECRET_ACCESS_KEY`) still
    /// produces a [`SesCredentials`] with the other half left empty.
    fn apply_env(&mut self) {
        if let Some(key) = env_non_empty("POSTMARK_API_KEY") {
            self.postmark = Some(key);
        }
        if let Some(key) = env_non_empty("RESEND_API_KEY") {
            self.resend = Some(key);
        }

        let ses_key = env_non_empty("AWS_ACCESS_KEY_ID");
        let ses_secret = env_non_empty("AWS_SECRET_ACCESS_KEY");
        if ses_key.is_some() || ses_secret.is_some() {
            let ses = self.ses.get_or_insert_with(|| SesCredentials {
                region: DEFAULT_SES_REGION.to_string(),
                ..SesCredentials::default()
            });
            if let Some(key) = ses_key {
                ses.key = key;
            }
            if let Some(secret) = ses_secret {
                ses.secret = secret;
            }
        }

        let slack_token = env_non_empty("SLACK_BOT_USER_OAUTH_TOKEN");
        let slack_channel = env_non_empty("SLACK_BOT_USER_DEFAULT_CHANNEL");
        if slack_token.is_some() || slack_channel.is_some() {
            let slack = self.slack.get_or_insert_with(SlackNotifications::default);
            if let Some(token) = slack_token {
                slack.bot_user_oauth_token = token;
            }
            if let Some(channel) = slack_channel {
                slack.channel = channel;
            }
        }
    }

    /// Drop composite blocks that carry no value, so accessors return `None`
    /// for a fully blank `[services.ses]` / `[services.slack.notifications]`.
    fn normalize(&mut self) {
        if let Some(ses) = &self.ses {
            if ses.key.is_empty() && ses.secret.is_empty() {
                self.ses = None;
            }
        }
        if let Some(slack) = &self.slack {
            if slack.bot_user_oauth_token.is_empty() && slack.channel.is_empty() {
                self.slack = None;
            }
        }
    }
}

/// Parse the `[services.ses]` block, defaulting a blank region.
fn parse_ses(services: &Map<String, Value>) -> Result<Option<SesCredentials>, ServicesConfigError> {
    let Some(table) = sub_table(services, "ses")? else {
        return Ok(None);
    };
    let key = string_field(table, "key", "services.ses.key")?.unwrap_or_default();
    let secret = string_field(table, "secret", "services.ses.secret")?.unwrap_or_default();
    let region = string_field(table, "region", "services.ses.region")?
        .unwrap_or_else(|| DEFAULT_SES_REGION.to_string());
    Ok(Some(SesCredentials {
        key,
        secret,
        region,
    }))
}

/// Parse the `[services.slack.notifications]` block.
fn parse_slack(
    services: &Map<String, Value>,
) -> Result<Option<SlackNotifications>, ServicesConfigError> {
    let Some(slack) = sub_table(services, "slack")? else {
        return Ok(None);
    };
    let Some(notifications) = sub_table(slack, "notifications")? else {
        return Ok(None);
    };
    let bot_user_oauth_token = string_field(
        notifications,
        "bot_user_oauth_token",
        "services.slack.notifications.bot_user_oauth_token",
    )?
    .unwrap_or_default();
    let channel = string_field(
        notifications,
        "channel",
        "services.slack.notifications.channel",
    )?
    .unwrap_or_default();
    Ok(Some(SlackNotifications {
        bot_user_oauth_token,
        channel,
    }))
}

/// Fetch a nested table, distinguishing "absent" from "wrong type".
fn sub_table<'a>(
    table: &'a Map<String, Value>,
    key: &str,
) -> Result<Option<&'a Map<String, Value>>, ServicesConfigError> {
    match table.get(key) {
        None => Ok(None),
        Some(value) => match &value.kind {
            ValueKind::Table(inner) => Ok(Some(inner)),
            ValueKind::Nil => Ok(None),
            other => Err(ServicesConfigError::Invalid(format!(
                "`services.{key}` must be a table, found {}",
                kind_label(other)
            ))),
        },
    }
}

/// Read a string leaf, treating blank as absent and rejecting non-strings.
fn string_field(
    table: &Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<Option<String>, ServicesConfigError> {
    match table.get(key) {
        None => Ok(None),
        Some(value) => match &value.kind {
            ValueKind::String(text) => Ok(non_empty(text)),
            ValueKind::Nil => Ok(None),
            other => Err(ServicesConfigError::Invalid(format!(
                "`{path}` must be a string, found {}",
                kind_label(other)
            ))),
        },
    }
}

/// True when a loader error means the requested key does not exist.
fn is_not_found(error: &config::ConfigError) -> bool {
    matches!(error, config::ConfigError::NotFound(_))
}

/// A human-readable label for a value kind, used in error messages.
fn kind_label(kind: &ValueKind) -> &'static str {
    match kind {
        ValueKind::Nil => "null",
        ValueKind::Boolean(_) => "a boolean",
        ValueKind::I64(_) | ValueKind::I128(_) | ValueKind::U64(_) | ValueKind::U128(_) => {
            "a number"
        }
        ValueKind::Float(_) => "a number",
        ValueKind::String(_) => "a string",
        ValueKind::Table(_) => "a table",
        ValueKind::Array(_) => "an array",
    }
}

/// Return `Some(trimmed)` when `value` is non-blank, else `None`.
fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Read an environment variable, treating unset or blank values as absent.
fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key).ok().and_then(|value| non_empty(&value))
}
