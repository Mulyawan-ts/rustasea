//! Typed Pusher config and the client-auth response (ADOPT-022).

use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize};

use crate::error::{BroadcastError, Result};

use super::{DEFAULT_PORT, DEFAULT_SCHEME, DEFAULT_TIMEOUT_SECS};

/// Serde default for [`PusherConfig::timeout`].
fn default_timeout() -> Duration {
    Duration::from_secs(DEFAULT_TIMEOUT_SECS)
}

/// Deserialize the `timeout` field as whole seconds into a [`Duration`].
fn deserialize_timeout<'de, D>(deserializer: D) -> std::result::Result<Duration, D::Error>
where
    D: Deserializer<'de>,
{
    let secs = Option::<u64>::deserialize(deserializer)?;
    Ok(Duration::from_secs(secs.unwrap_or(DEFAULT_TIMEOUT_SECS)))
}

/// Typed `[broadcasting.connections.pusher]` configuration.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct PusherConfig {
    /// Pusher application id.
    #[serde(default)]
    pub app_id: String,
    /// Pusher application key (public).
    #[serde(default)]
    pub key: String,
    /// Pusher application secret (used to sign requests).
    #[serde(default)]
    pub secret: String,
    /// Pusher cluster (`us2`, `eu`, …); used when `host` is unset.
    #[serde(default)]
    pub cluster: Option<String>,
    /// Self-hosted host; when set, overrides the cluster endpoint.
    #[serde(default)]
    pub host: Option<String>,
    /// Self-hosted port (defaults to 6001 when `host` is set).
    #[serde(default)]
    pub port: Option<u16>,
    /// Self-hosted scheme (defaults to `http` when `host` is set).
    #[serde(default)]
    pub scheme: Option<String>,
    /// Per-request HTTP timeout (seconds in the config file).
    #[serde(default = "default_timeout", deserialize_with = "deserialize_timeout")]
    pub timeout: Duration,
}

impl Default for PusherConfig {
    /// An empty config with the default timeout.
    fn default() -> Self {
        Self {
            app_id: String::new(),
            key: String::new(),
            secret: String::new(),
            cluster: None,
            host: None,
            port: None,
            scheme: None,
            timeout: default_timeout(),
        }
    }
}

impl std::fmt::Debug for PusherConfig {
    /// Render the config with the `secret` masked so a debug dump or log line
    /// can never leak the signing credential (security hygiene, ADOPT-022).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PusherConfig")
            .field("app_id", &self.app_id)
            .field("key", &self.key)
            .field("secret", &"[REDACTED]")
            .field("cluster", &self.cluster)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("scheme", &self.scheme)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl PusherConfig {
    /// Whether the config has the credentials required to sign a request.
    ///
    /// A config is complete once `app_id`, `key`, and `secret` are non-blank and
    /// an endpoint can be resolved (`host` or `cluster` present). Partial
    /// credentials therefore surface [`BroadcastError::NotConfigured`] rather
    /// than being accepted.
    pub fn is_complete(&self) -> bool {
        !self.app_id.trim().is_empty()
            && !self.key.trim().is_empty()
            && !self.secret.trim().is_empty()
            && self.endpoint().is_ok()
    }

    /// The `POST .../events` endpoint URL (no query string).
    ///
    /// # Errors
    ///
    /// [`BroadcastError::NotConfigured`] when neither `host` nor `cluster` is
    /// set, so no endpoint can be resolved.
    pub fn endpoint(&self) -> Result<String> {
        if let Some(host) = self.host.as_deref().filter(|h| !h.trim().is_empty()) {
            let scheme = self
                .scheme
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(DEFAULT_SCHEME);
            let port = self.port.unwrap_or(DEFAULT_PORT);
            return Ok(format!(
                "{scheme}://{host}:{port}/apps/{}/events",
                self.app_id
            ));
        }
        match self.cluster.as_deref().filter(|c| !c.trim().is_empty()) {
            Some(cluster) => Ok(format!(
                "https://api-{cluster}.pusher.com/apps/{}/events",
                self.app_id
            )),
            None => Err(BroadcastError::NotConfigured {
                connection: "pusher".to_string(),
            }),
        }
    }

    /// The path component used in the signature string-to-sign.
    pub fn events_path(&self) -> String {
        format!("/apps/{}/events", self.app_id)
    }
}

/// The client-side channel authorization response (`POST /broadcasting/auth`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelAuth {
    /// The `{key}:{signature}` auth token the client sends to Pusher.
    pub auth: String,
    /// The JSON-encoded presence member data (presence channels only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_data: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The debug rendering masks the secret but keeps the other fields visible.
    #[test]
    fn debug_redacts_secret() {
        let config = PusherConfig {
            app_id: "123456".to_string(),
            key: "app-key".to_string(),
            secret: "super-secret-value".to_string(),
            cluster: Some("us2".to_string()),
            ..PusherConfig::default()
        };
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("super-secret-value"));
        assert!(rendered.contains("[REDACTED]"));
        assert!(rendered.contains("123456"));
        assert!(rendered.contains("app-key"));
        assert!(rendered.contains("us2"));
    }
}
