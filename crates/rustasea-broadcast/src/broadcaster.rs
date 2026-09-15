//! Broadcast driver abstraction (ADOPT-022).
//!
//! A [`Broadcaster`] is one named connection that can push a
//! [`BroadcastPayload`] to subscribers. The in-process [`BroadcastHub`] is the
//! default implementation (behind the `ws` feature); the external drivers
//! (`pusher`, `redis`) implement the same trait so [`crate::manager`] can treat
//! every connection uniformly.
//!
//! The trait is object-safe (`Arc<dyn Broadcaster>`), async (`#[async_trait]`),
//! and infallible at construction — a driver that cannot reach its backend
//! surfaces a typed [`BroadcastError`] from [`Broadcaster::publish`] rather than
//! silently dropping the payload.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// One resolved broadcast payload, ready for a driver to deliver.
///
/// The `channel` is the **wire** channel name (already prefixed with
/// `private-`/`presence-` where applicable), and `data` is the serialized event
/// body as a JSON value (drivers stringify it per their protocol).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BroadcastPayload {
    /// Event name reported to subscribers.
    pub event: String,
    /// Wire channel name (`private-chat.1`, `orders.1`, …).
    pub channel: String,
    /// Serialized event body.
    pub data: serde_json::Value,
}

impl BroadcastPayload {
    /// Build a payload from its parts.
    pub fn new(
        event: impl Into<String>,
        channel: impl Into<String>,
        data: serde_json::Value,
    ) -> Self {
        Self {
            event: event.into(),
            channel: channel.into(),
            data,
        }
    }

    /// Serialize the payload to a compact JSON string (the wire body most
    /// drivers transmit).
    ///
    /// # Errors
    ///
    /// [`BroadcastError::Serialization`](crate::error::BroadcastError::Serialization)
    /// when the value cannot be encoded.
    pub fn to_json_string(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    /// Parse a payload back from a JSON string (the subscriber side).
    ///
    /// # Errors
    ///
    /// [`BroadcastError::Serialization`](crate::error::BroadcastError::Serialization)
    /// when the string is not a valid payload.
    pub fn from_json_str(json: &str) -> Result<Self> {
        Ok(serde_json::from_str(json)?)
    }
}

/// A named broadcast connection that can deliver a payload.
///
/// Implementations are shared behind an `Arc` and must be `Send + Sync`.
/// `connection` is the stable driver/connection label reported in errors and
/// introspection.
#[async_trait]
pub trait Broadcaster: Send + Sync + 'static {
    /// The connection label (`"hub"`, `"pusher"`, `"redis"`, …).
    fn connection(&self) -> &'static str;

    /// Deliver `payload` to its channel.
    ///
    /// # Errors
    ///
    /// A driver-specific [`BroadcastError`] on failure; a driver must never
    /// return `Ok` for a dropped payload.
    async fn publish(&self, payload: BroadcastPayload) -> Result<()>;
}

#[cfg(feature = "ws")]
#[async_trait]
impl Broadcaster for crate::hub::BroadcastHub {
    /// The in-process hub connection label.
    fn connection(&self) -> &'static str {
        "hub"
    }

    /// Serialize the payload and fan it out through the local hub.
    ///
    /// The hub's [`WsMessage`](crate::hub::WsMessage) carries `data` as a
    /// string, so the JSON value is stringified here.
    async fn publish(&self, payload: BroadcastPayload) -> Result<()> {
        let data = serde_json::to_string(&payload.data)?;
        let message = crate::hub::WsMessage::new(payload.event, payload.channel.clone(), data);
        self.publish(&payload.channel, message).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A payload round-trips through its JSON string form.
    #[test]
    fn payload_json_round_trip() {
        let payload = BroadcastPayload::new(
            "UserCreated",
            "private-chat.1",
            serde_json::json!({ "name": "Ada" }),
        );
        let encoded = payload.to_json_string().expect("encode");
        let decoded = BroadcastPayload::from_json_str(&encoded).expect("decode");
        assert_eq!(decoded, payload);
        assert_eq!(decoded.channel, "private-chat.1");
        assert_eq!(decoded.data["name"], "Ada");
    }

    /// A malformed string is a typed serialization error, not a panic.
    #[test]
    fn payload_from_invalid_json_is_error() {
        let err = BroadcastPayload::from_json_str("not json").unwrap_err();
        assert!(matches!(
            err,
            crate::error::BroadcastError::Serialization(_)
        ));
    }
}
