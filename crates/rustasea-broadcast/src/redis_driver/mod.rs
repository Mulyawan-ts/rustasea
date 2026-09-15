//! Redis Pub/Sub broadcast driver (ADOPT-022).
//!
//! Parity target: Laravel's `redis` broadcast driver. [`RedisBroadcaster`]
//! publishes each payload to the Redis channel `{prefix}.{wire_channel}`, and
//! [`RedisSubscriber`] re-publishes received messages into a local
//! [`BroadcastHub`](crate::hub::BroadcastHub) so every process in the fleet sees
//! the event.
//!
//! The Redis client is hidden behind the [`RedisPubSub`] seam so the fan-out is
//! testable with an in-memory bus (no live Redis in unit tests).

#[cfg(feature = "ws")]
pub mod subscriber;

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::broadcaster::{BroadcastPayload, Broadcaster};
use crate::error::{BroadcastError, Result};

#[cfg(feature = "ws")]
pub use subscriber::RedisSubscriber;

/// Default channel prefix (Laravel's `broadcasting` prefix parity).
pub const DEFAULT_PREFIX: &str = "broadcasting";

/// The Redis connection label.
const CONNECTION: &str = "redis";

/// Serde default for [`RedisConfig::prefix`].
fn default_prefix() -> String {
    DEFAULT_PREFIX.to_string()
}

/// Typed `[broadcasting.connections.redis]` configuration.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct RedisConfig {
    /// Redis connection URL (`redis://127.0.0.1:6379`).
    #[serde(default)]
    pub url: String,
    /// Channel prefix; channels are `{prefix}.{wire_channel}`.
    #[serde(default = "default_prefix")]
    pub prefix: String,
}

impl std::fmt::Debug for RedisConfig {
    /// Render the config with any URL password masked so a debug dump or log
    /// line can never leak the Redis credential (security hygiene, ADOPT-022).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisConfig")
            .field("url", &mask_url_password(&self.url))
            .field("prefix", &self.prefix)
            .finish()
    }
}

/// Mask the password component of a URL's userinfo (`user:pass@` → `user:***@`).
///
/// A URL without a userinfo password (or without userinfo at all) is returned
/// unchanged, so only the secret segment is redacted.
fn mask_url_password(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    // The authority runs until the path/query/fragment, whichever comes first.
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    let Some(at) = authority.rfind('@') else {
        return url.to_string();
    };
    let (userinfo, host) = authority.split_at(at);
    let masked_userinfo = match userinfo.split_once(':') {
        Some((user, _password)) => format!("{user}:***"),
        None => userinfo.to_string(),
    };
    format!("{scheme}://{masked_userinfo}{host}{tail}")
}

impl Default for RedisConfig {
    /// An empty URL with the default `broadcasting` prefix.
    fn default() -> Self {
        Self {
            url: String::new(),
            prefix: default_prefix(),
        }
    }
}

impl RedisConfig {
    /// Whether the config has the URL required to connect.
    pub fn is_complete(&self) -> bool {
        !self.url.trim().is_empty()
    }

    /// The Redis channel for a wire channel (`{prefix}.{wire_channel}`).
    pub fn channel_for(&self, wire_channel: &str) -> String {
        format!("{}.{wire_channel}", self.prefix)
    }

    /// The Pub/Sub subscription pattern (`{prefix}.*`).
    pub fn pattern(&self) -> String {
        format!("{}.*", self.prefix)
    }
}

/// The Redis Pub/Sub seam.
///
/// The production implementation ([`RedisClientBus`]) wraps a `redis::Client`;
/// tests install an in-memory bus so the fan-out runs without a live Redis.
#[async_trait]
pub trait RedisPubSub: Send + Sync + 'static {
    /// Publish `payload` to Redis `channel`.
    ///
    /// # Errors
    ///
    /// [`BroadcastError::Driver`] on a connection or command failure.
    async fn publish(&self, channel: &str, payload: &str) -> Result<()>;

    /// Subscribe to the glob `pattern`, returning a stream of
    /// `(channel, payload)` messages.
    ///
    /// # Errors
    ///
    /// [`BroadcastError::Driver`] when the subscription cannot be established.
    async fn psubscribe(&self, pattern: &str) -> Result<mpsc::Receiver<(String, String)>>;
}

/// `redis`-crate-backed Pub/Sub bus.
pub struct RedisClientBus {
    client: redis::Client,
}

impl RedisClientBus {
    /// Open a client for `url`.
    ///
    /// # Errors
    ///
    /// [`BroadcastError::Driver`] when the URL is not a valid Redis URL.
    pub fn open(url: &str) -> Result<Self> {
        let client = redis::Client::open(url).map_err(|error| BroadcastError::Driver {
            connection: CONNECTION.to_string(),
            message: format!("invalid redis url: {error}"),
        })?;
        Ok(Self { client })
    }
}

#[async_trait]
impl RedisPubSub for RedisClientBus {
    /// `PUBLISH {channel} {payload}` over a multiplexed async connection.
    async fn publish(&self, channel: &str, payload: &str) -> Result<()> {
        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|error| BroadcastError::Driver {
                connection: CONNECTION.to_string(),
                message: format!("connection failed: {error}"),
            })?;
        let _: i64 = redis::cmd("PUBLISH")
            .arg(channel)
            .arg(payload)
            .query_async(&mut conn)
            .await
            .map_err(|error| BroadcastError::Driver {
                connection: CONNECTION.to_string(),
                message: format!("publish failed: {error}"),
            })?;
        Ok(())
    }

    /// `PSUBSCRIBE {pattern}` and forward messages into an mpsc channel.
    async fn psubscribe(&self, pattern: &str) -> Result<mpsc::Receiver<(String, String)>> {
        use futures_util::StreamExt;

        let mut pubsub =
            self.client
                .get_async_pubsub()
                .await
                .map_err(|error| BroadcastError::Driver {
                    connection: CONNECTION.to_string(),
                    message: format!("pubsub connection failed: {error}"),
                })?;
        pubsub
            .psubscribe(pattern)
            .await
            .map_err(|error| BroadcastError::Driver {
                connection: CONNECTION.to_string(),
                message: format!("psubscribe failed: {error}"),
            })?;

        let (tx, rx) = mpsc::channel::<(String, String)>(256);
        let mut stream = pubsub.into_on_message();
        tokio::spawn(async move {
            while let Some(msg) = stream.next().await {
                let channel = msg.get_channel_name().to_string();
                let payload: String = msg.get_payload().unwrap_or_default();
                if tx.send((channel, payload)).await.is_err() {
                    break;
                }
            }
        });
        Ok(rx)
    }
}

/// The Redis Pub/Sub broadcast driver.
pub struct RedisBroadcaster {
    /// Resolved configuration.
    config: RedisConfig,
    /// The Pub/Sub bus.
    bus: Arc<dyn RedisPubSub>,
}

impl RedisBroadcaster {
    /// Build a driver with a live Redis client bus.
    ///
    /// # Errors
    ///
    /// [`BroadcastError::Driver`] when the configured URL is invalid.
    pub fn new(config: RedisConfig) -> Result<Self> {
        let bus = Arc::new(RedisClientBus::open(&config.url)?);
        Ok(Self { config, bus })
    }

    /// Build a driver with a caller-supplied bus (test seam).
    pub fn with_bus(config: RedisConfig, bus: Arc<dyn RedisPubSub>) -> Self {
        Self { config, bus }
    }

    /// The driver configuration.
    pub fn config(&self) -> &RedisConfig {
        &self.config
    }

    /// The shared Pub/Sub bus (so a subscriber can reuse it).
    pub fn bus(&self) -> Arc<dyn RedisPubSub> {
        Arc::clone(&self.bus)
    }
}

#[async_trait]
impl Broadcaster for RedisBroadcaster {
    /// The Redis connection label.
    fn connection(&self) -> &'static str {
        CONNECTION
    }

    /// Publish the payload to `{prefix}.{wire_channel}`.
    async fn publish(&self, payload: BroadcastPayload) -> Result<()> {
        let channel = self.config.channel_for(&payload.channel);
        let body = payload.to_json_string()?;
        self.bus.publish(&channel, &body).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The channel name and pattern use the configured prefix.
    #[test]
    fn channel_and_pattern_use_prefix() {
        let config = RedisConfig {
            url: "redis://localhost".to_string(),
            prefix: "app".to_string(),
        };
        assert_eq!(config.channel_for("private-chat.1"), "app.private-chat.1");
        assert_eq!(config.pattern(), "app.*");
        assert!(config.is_complete());
    }

    /// A blank URL is incomplete.
    #[test]
    fn blank_url_is_incomplete() {
        assert!(!RedisConfig::default().is_complete());
    }

    /// The debug rendering masks the URL password but keeps the rest visible.
    #[test]
    fn debug_redacts_url_password() {
        let config = RedisConfig {
            url: "redis://user:sup3rsecret@redis.example:6379/0".to_string(),
            prefix: "app".to_string(),
        };
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("sup3rsecret"));
        assert!(rendered.contains("redis://user:***@redis.example:6379/0"));
        assert!(rendered.contains("app"));
    }

    /// A URL without a password renders unchanged, and so does a bare host.
    #[test]
    fn debug_leaves_passwordless_urls_intact() {
        let no_password = RedisConfig {
            url: "redis://redis.example:6379".to_string(),
            prefix: "broadcasting".to_string(),
        };
        assert!(format!("{no_password:?}").contains("redis://redis.example:6379"));

        let user_only = RedisConfig {
            url: "redis://user@redis.example:6379".to_string(),
            prefix: "broadcasting".to_string(),
        };
        assert!(format!("{user_only:?}").contains("redis://user@redis.example:6379"));
    }
}
