//! The named-connection registry and dispatch façade (ADOPT-022).
//!
//! [`BroadcastManager`] owns the built [`Broadcaster`] connections plus the
//! authorization gate, and builds itself from a resolved
//! [`BroadcastingConfig`](super::BroadcastingConfig).

use std::collections::HashMap;
use std::sync::Arc;

use crate::broadcaster::{BroadcastPayload, Broadcaster};
use crate::channel::{authorize_subscription, Authorize, Channel, Subscriber};
use crate::error::{BroadcastError, Result};

#[cfg(feature = "ws")]
use super::CONNECTION_HUB;
use super::{BroadcastingConfig, CONNECTION_PUSHER, CONNECTION_REDIS};

/// A manager of named [`Broadcaster`] connections plus the authorization gate.
///
/// The app builds one at boot from
/// [`BroadcastingConfig::from_loader`](super::BroadcastingConfig::from_loader)
/// and installs it process-wide; handlers publish through it and authorize
/// channel subscriptions against the same gate the hub uses.
pub struct BroadcastManager {
    /// Connection used when a publish names none.
    default: String,
    /// Built connections keyed by name.
    connections: HashMap<String, Arc<dyn Broadcaster>>,
    /// Channel authorization gate (shared with the in-process hub).
    gate: Option<Arc<dyn Authorize>>,
    /// Pusher config retained for client-side channel authorization.
    #[cfg(feature = "pusher")]
    pusher: Option<crate::pusher::PusherConfig>,
}

impl BroadcastManager {
    /// Create an empty manager with `default` as the default connection.
    pub fn new(default: impl Into<String>, gate: Option<Arc<dyn Authorize>>) -> Self {
        Self {
            default: default.into(),
            connections: HashMap::new(),
            gate,
            #[cfg(feature = "pusher")]
            pusher: None,
        }
    }

    /// Attach the Pusher config used for client-side channel authorization.
    #[cfg(feature = "pusher")]
    pub fn with_pusher_config(mut self, config: crate::pusher::PusherConfig) -> Self {
        self.pusher = Some(config);
        self
    }

    /// The Pusher config, when one is configured (`pusher` feature).
    #[cfg(feature = "pusher")]
    pub fn pusher_config(&self) -> Option<&crate::pusher::PusherConfig> {
        self.pusher.as_ref()
    }

    /// Register a connection, replacing any existing one of the same name.
    pub fn register(&mut self, name: impl Into<String>, broadcaster: Arc<dyn Broadcaster>) {
        self.connections.insert(name.into(), broadcaster);
    }

    /// The default connection name.
    pub fn default_name(&self) -> &str {
        &self.default
    }

    /// Look up a connection by name.
    ///
    /// # Errors
    ///
    /// [`BroadcastError::ConnectionUnknown`] when no such connection is
    /// registered.
    pub fn connection(&self, name: &str) -> Result<&Arc<dyn Broadcaster>> {
        self.connections
            .get(name)
            .ok_or_else(|| BroadcastError::ConnectionUnknown {
                connection: name.to_string(),
            })
    }

    /// The default connection.
    ///
    /// # Errors
    ///
    /// [`BroadcastError::ConnectionUnknown`] when the default is not registered.
    pub fn default_connection(&self) -> Result<&Arc<dyn Broadcaster>> {
        self.connection(&self.default)
    }

    /// The installed authorization gate, if any.
    pub fn gate(&self) -> Option<&Arc<dyn Authorize>> {
        self.gate.as_ref()
    }

    /// Publish a payload on the default connection.
    ///
    /// # Errors
    ///
    /// [`BroadcastError::ConnectionUnknown`] when the default is unregistered,
    /// or the driver's own error.
    pub async fn publish(&self, payload: BroadcastPayload) -> Result<()> {
        self.default_connection()?.publish(payload).await
    }

    /// Serialize a broadcastable event and publish it on the default connection.
    ///
    /// # Errors
    ///
    /// [`BroadcastError::Serialization`] when the event cannot be encoded, or
    /// the connection/driver error from [`BroadcastManager::publish`].
    pub async fn dispatch<E>(&self, event: &E) -> Result<()>
    where
        E: crate::ShouldBroadcast + crate::BroadcastEvent + serde::Serialize,
    {
        let channel = event.broadcast_on();
        let wire = crate::to_wire(event.event_name(), &channel, event)?;
        let payload = BroadcastPayload {
            event: event.event_name().to_string(),
            channel: channel.auth_channel(),
            data: wire.get("data").cloned().unwrap_or(serde_json::Value::Null),
        };
        self.publish(payload).await
    }

    /// Authorize `subscriber` for the wire channel `channel_wire`.
    ///
    /// Public channels always pass; private/presence channels run the installed
    /// gate. A missing gate or subscriber on a gated channel surfaces
    /// [`BroadcastError::Unauthenticated`]; a denial surfaces
    /// [`BroadcastError::Unauthorized`].
    ///
    /// # Errors
    ///
    /// [`BroadcastError::Unauthenticated`] / [`BroadcastError::Unauthorized`] as
    /// described above.
    pub async fn authorize(&self, channel_wire: &str, subscriber: &Subscriber) -> Result<()> {
        let channel = channel_from_wire(channel_wire);
        authorize_subscription(channel, Some(subscriber), self.gate.as_deref())
            .await
            .map(|_| ())
    }

    /// Authorize `subscriber` for `channel_wire` and, on success, produce the
    /// Pusher client-auth signature for `socket_id` (`pusher` feature).
    ///
    /// Public channels are authorized without a gate; private/presence channels
    /// run the gate and are signed against `channel_data` (presence).
    ///
    /// # Errors
    ///
    /// [`BroadcastError::Unauthenticated`] / [`BroadcastError::Unauthorized`] from
    /// [`BroadcastManager::authorize`], or
    /// [`BroadcastError::NotConfigured`] when no Pusher config is installed, or
    /// [`BroadcastError::Transport`] if signing fails.
    #[cfg(feature = "pusher")]
    pub async fn pusher_channel_auth(
        &self,
        socket_id: &str,
        channel_wire: &str,
        subscriber: &Subscriber,
        channel_data: Option<&str>,
    ) -> Result<crate::pusher::ChannelAuth> {
        self.authorize(channel_wire, subscriber).await?;
        let config = self
            .pusher
            .as_ref()
            .ok_or_else(|| BroadcastError::NotConfigured {
                connection: CONNECTION_PUSHER.to_string(),
            })?;
        let broadcaster = crate::pusher::PusherBroadcaster::new(config.clone());
        broadcaster.auth_signature(socket_id, channel_wire, channel_data)
    }

    /// Build a manager from a resolved [`BroadcastingConfig`].
    ///
    /// Always registers the in-process `hub` connection (with the `ws` feature).
    /// The `pusher` and `redis` drivers are registered when configured. A driver
    /// that is *selected as the default* but has no (or an incomplete)
    /// configuration surfaces [`BroadcastError::NotConfigured`]; a configured
    /// non-default driver that is incomplete is likewise rejected so a partial
    /// credential set never silently drops external delivery.
    ///
    /// # Errors
    ///
    /// [`BroadcastError::NotConfigured`] as described above.
    pub fn from_config(
        config: &BroadcastingConfig,
        gate: Option<Arc<dyn Authorize>>,
    ) -> Result<Self> {
        // With no optional driver feature enabled, `from_config` registers no
        // connection, so the binding stays immutable (avoids `unused_mut` under
        // `--no-default-features`).
        #[cfg(not(any(feature = "ws", feature = "pusher", feature = "redis")))]
        let manager = Self::new(config.default.clone(), gate);
        #[cfg(any(feature = "ws", feature = "pusher", feature = "redis"))]
        let mut manager = Self::new(config.default.clone(), gate);

        #[cfg(feature = "ws")]
        {
            let hub_gate: Arc<dyn Authorize> = manager
                .gate
                .clone()
                .unwrap_or_else(|| Arc::new(DenyAllGate));
            let hub = crate::hub::BroadcastHub::new(SharedGate(hub_gate));
            manager.register(CONNECTION_HUB, Arc::new(hub));
        }

        #[cfg(feature = "pusher")]
        {
            if let Some(pusher) = &config.pusher {
                if !pusher.is_complete() {
                    return Err(BroadcastError::NotConfigured {
                        connection: CONNECTION_PUSHER.to_string(),
                    });
                }
                manager.register(
                    CONNECTION_PUSHER,
                    Arc::new(crate::pusher::PusherBroadcaster::new(pusher.clone())),
                );
                manager.pusher = Some(pusher.clone());
            } else if config.default == CONNECTION_PUSHER {
                return Err(BroadcastError::NotConfigured {
                    connection: CONNECTION_PUSHER.to_string(),
                });
            }
        }
        #[cfg(not(feature = "pusher"))]
        if config.default == CONNECTION_PUSHER {
            return Err(BroadcastError::NotConfigured {
                connection: CONNECTION_PUSHER.to_string(),
            });
        }

        #[cfg(feature = "redis")]
        {
            if let Some(redis) = &config.redis {
                if !redis.is_complete() {
                    return Err(BroadcastError::NotConfigured {
                        connection: CONNECTION_REDIS.to_string(),
                    });
                }
                // `new` only validates the URL (no connection is opened here),
                // so a failure means the URL is malformed → `NotConfigured`.
                let broadcaster = crate::redis_driver::RedisBroadcaster::new(redis.clone())
                    .map_err(|_| BroadcastError::NotConfigured {
                        connection: CONNECTION_REDIS.to_string(),
                    })?;
                manager.register(CONNECTION_REDIS, Arc::new(broadcaster));
            } else if config.default == CONNECTION_REDIS {
                return Err(BroadcastError::NotConfigured {
                    connection: CONNECTION_REDIS.to_string(),
                });
            }
        }
        #[cfg(not(feature = "redis"))]
        if config.default == CONNECTION_REDIS {
            return Err(BroadcastError::NotConfigured {
                connection: CONNECTION_REDIS.to_string(),
            });
        }

        // The default must resolve to a built connection; `hub` requires `ws`.
        if manager.connection(&config.default).is_err() {
            return Err(BroadcastError::NotConfigured {
                connection: config.default.clone(),
            });
        }

        Ok(manager)
    }
}

/// Derive a [`Channel`] kind from a wire name by prefix (mirrors the hub).
fn channel_from_wire(wire_channel: &str) -> Channel {
    if let Some(name) = wire_channel.strip_prefix("private-") {
        Channel::Private(name.to_string())
    } else if let Some(name) = wire_channel.strip_prefix("presence-") {
        Channel::Presence(name.to_string())
    } else {
        Channel::Public(wire_channel.to_string())
    }
}

/// Authorization gate used when no gate is installed for the in-process hub.
#[cfg(feature = "ws")]
struct DenyAllGate;

#[cfg(feature = "ws")]
#[async_trait::async_trait]
impl Authorize for DenyAllGate {
    /// Deny every private/presence subscription (no gate installed).
    async fn authorize(&self, _channel: &str, _identity: &str) -> crate::channel::AuthDecision {
        crate::channel::AuthDecision::Deny("no broadcast authorization gate installed".to_string())
    }
}

/// Adapter letting an `Arc<dyn Authorize>` satisfy the hub's `impl Authorize`.
#[cfg(feature = "ws")]
struct SharedGate(Arc<dyn Authorize>);

#[cfg(feature = "ws")]
#[async_trait::async_trait]
impl Authorize for SharedGate {
    /// Delegate to the shared gate.
    async fn authorize(&self, channel: &str, identity: &str) -> crate::channel::AuthDecision {
        self.0.authorize(channel, identity).await
    }
}
