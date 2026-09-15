//! Redis Pub/Sub → local hub fan-out (ADOPT-022).
//!
//! [`RedisSubscriber`] subscribes to `{prefix}.*` and re-publishes every message
//! it receives into a local [`BroadcastHub`](crate::hub::BroadcastHub), so an
//! event published in one process reaches the WebSocket subscribers of every
//! process in the fleet.
//!
//! Gated on both the `redis` and `ws` features: it needs the Redis bus and the
//! in-process hub.

use std::sync::Arc;

use tokio::task::JoinHandle;

use crate::broadcaster::BroadcastPayload;
use crate::error::Result;
use crate::hub::{BroadcastHub, WsMessage};
use crate::redis_driver::{RedisConfig, RedisPubSub};

/// A running Redis → hub fan-out task.
pub struct RedisSubscriber {
    /// The spawned background task.
    handle: JoinHandle<()>,
}

impl RedisSubscriber {
    /// Start a subscriber that forwards `{prefix}.*` messages into `hub`.
    ///
    /// The task runs until the underlying Pub/Sub stream closes or the bus is
    /// dropped; the returned handle can be awaited or aborted by the caller.
    ///
    /// # Errors
    ///
    /// [`BroadcastError::Driver`](crate::error::BroadcastError::Driver) when the
    /// subscription cannot be established.
    pub async fn start(
        hub: BroadcastHub,
        config: RedisConfig,
        bus: Arc<dyn RedisPubSub>,
    ) -> Result<Self> {
        let mut rx = bus.psubscribe(&config.pattern()).await?;
        let prefix = format!("{}.", config.prefix);
        let handle = tokio::spawn(async move {
            while let Some((channel, body)) = rx.recv().await {
                let Ok(payload) = BroadcastPayload::from_json_str(&body) else {
                    continue;
                };
                // Strip the configured prefix to recover the wire channel.
                let wire = channel.strip_prefix(&prefix).unwrap_or(&channel);
                let data = serde_json::to_string(&payload.data).unwrap_or_default();
                let message = WsMessage::new(payload.event, wire, data);
                // Best-effort: a hub with no subscribers for the channel is a
                // no-op, never an error.
                let _ = hub.publish(wire, message).await;
            }
        });
        Ok(Self { handle })
    }

    /// A handle to the spawned fan-out task.
    pub fn handle(&self) -> &JoinHandle<()> {
        &self.handle
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use tokio::sync::mpsc;

    use crate::broadcaster::Broadcaster;
    use crate::channel::{AuthDecision, Authorize};
    use crate::redis_driver::RedisBroadcaster;

    /// An in-memory Pub/Sub bus shared between a publisher and subscribers.
    ///
    /// Each `psubscribe` registers a fresh sender; `publish` fans out to every
    /// registered subscriber whose pattern matches. This models Redis Pub/Sub
    /// without a live server.
    #[derive(Default)]
    struct FakeBus {
        subscribers: Mutex<Vec<(String, mpsc::Sender<(String, String)>)>>,
    }

    impl FakeBus {
        /// Whether `pattern` (only `prefix.*` is used here) matches `channel`.
        fn matches(pattern: &str, channel: &str) -> bool {
            match pattern.strip_suffix(".*") {
                Some(prefix) => channel.starts_with(&format!("{prefix}.")),
                None => pattern == channel,
            }
        }
    }

    #[async_trait::async_trait]
    impl RedisPubSub for FakeBus {
        async fn publish(&self, channel: &str, payload: &str) -> Result<()> {
            let mut guard = self.subscribers.lock().expect("bus lock");
            guard.retain(|(_, tx)| !tx.is_closed());
            for (pattern, tx) in guard.iter() {
                if Self::matches(pattern, channel) {
                    let _ = tx.try_send((channel.to_string(), payload.to_string()));
                }
            }
            Ok(())
        }

        async fn psubscribe(&self, pattern: &str) -> Result<mpsc::Receiver<(String, String)>> {
            let (tx, rx) = mpsc::channel(64);
            self.subscribers
                .lock()
                .expect("bus lock")
                .push((pattern.to_string(), tx));
            Ok(rx)
        }
    }

    /// A permissive gate so the test hub accepts private-channel publishes.
    struct AllowAll;

    #[async_trait::async_trait]
    impl Authorize for AllowAll {
        async fn authorize(&self, _channel: &str, _identity: &str) -> AuthDecision {
            AuthDecision::Allow
        }
    }

    /// Two broadcasters on one bus fan a message out to two hubs.
    #[tokio::test]
    async fn two_hubs_receive_a_published_payload() {
        let bus = Arc::new(FakeBus::default());
        let config = RedisConfig {
            url: "redis://unused".to_string(),
            prefix: "broadcasting".to_string(),
        };

        let hub_a = BroadcastHub::new(AllowAll);
        let hub_b = BroadcastHub::new(AllowAll);

        let _sub_a = RedisSubscriber::start(hub_a.clone(), config.clone(), bus.clone())
            .await
            .expect("subscriber A");
        let _sub_b = RedisSubscriber::start(hub_b.clone(), config.clone(), bus.clone())
            .await
            .expect("subscriber B");

        // Both hubs have a subscriber on the wire channel.
        let mut rx_a = hub_a
            .subscribe("user-1", "private-chat.1")
            .await
            .expect("subscribe A");
        let mut rx_b = hub_b
            .subscribe("user-1", "private-chat.1")
            .await
            .expect("subscribe B");

        let publisher = RedisBroadcaster::with_bus(config.clone(), bus.clone());
        publisher
            .publish(BroadcastPayload::new(
                "UserCreated",
                "private-chat.1",
                serde_json::json!({ "name": "Ada" }),
            ))
            .await
            .expect("publish");

        let msg_a = rx_a.recv().await.expect("hub A receives");
        let msg_b = rx_b.recv().await.expect("hub B receives");
        for msg in [&msg_a, &msg_b] {
            assert_eq!(msg.event, "UserCreated");
            assert_eq!(msg.channel, "private-chat.1");
            let data: serde_json::Value = serde_json::from_str(&msg.data).expect("data JSON");
            assert_eq!(data["name"], "Ada");
        }
    }

    /// The published body round-trips through the payload codec.
    #[tokio::test]
    async fn published_payload_round_trips() {
        let bus = Arc::new(FakeBus::default());
        let config = RedisConfig {
            url: "redis://unused".to_string(),
            prefix: "app".to_string(),
        };
        let mut rx = bus.psubscribe("app.*").await.expect("subscribe");

        let publisher = RedisBroadcaster::with_bus(config.clone(), bus.clone());
        let original = BroadcastPayload::new("Ping", "orders.1", serde_json::json!({ "n": 7 }));
        publisher.publish(original.clone()).await.expect("publish");

        let (channel, body) = rx.recv().await.expect("message");
        assert_eq!(channel, "app.orders.1");
        let decoded = BroadcastPayload::from_json_str(&body).expect("decode");
        assert_eq!(decoded, original);
    }
}
