//! The Pusher HTTP broadcast driver (ADOPT-022).

use std::sync::Arc;

use async_trait::async_trait;

use crate::broadcaster::{BroadcastPayload, Broadcaster};
use crate::error::{BroadcastError, Result};

use super::config::{ChannelAuth, PusherConfig};
use super::signing::{body_md5, channel_auth_string, hmac_sha256_hex, string_to_sign};
use super::transport::{PusherTransport, ReqwestTransport};
use super::AUTH_VERSION;

/// The Pusher HTTP broadcast driver.
pub struct PusherBroadcaster {
    /// Resolved configuration.
    config: PusherConfig,
    /// Swappable HTTP transport.
    transport: Arc<dyn PusherTransport>,
}

impl PusherBroadcaster {
    /// Build a driver with the production `reqwest` transport.
    pub fn new(config: PusherConfig) -> Self {
        let transport = Arc::new(ReqwestTransport::new(config.timeout));
        Self { config, transport }
    }

    /// Build a driver with a caller-supplied transport (test seam).
    pub fn with_transport(config: PusherConfig, transport: Arc<dyn PusherTransport>) -> Self {
        Self { config, transport }
    }

    /// The driver configuration.
    pub fn config(&self) -> &PusherConfig {
        &self.config
    }

    /// Build the signed URL + body for a payload.
    ///
    /// Exposed for tests (and reused by [`Broadcaster::publish`]) so the exact
    /// wire request can be asserted without a network.
    ///
    /// # Errors
    ///
    /// [`BroadcastError::NotConfigured`] when the endpoint cannot be resolved,
    /// or [`BroadcastError::Serialization`] / [`BroadcastError::Transport`] on
    /// encoding / signing failure.
    pub fn signed_request(&self, payload: &BroadcastPayload) -> Result<(String, String)> {
        let endpoint = self.config.endpoint()?;
        // `data` must be a JSON-encoded STRING per the Pusher spec.
        let data = serde_json::to_string(&payload.data)?;
        let body = serde_json::json!({
            "name": payload.event,
            "channels": [payload.channel],
            "data": data,
        })
        .to_string();

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0)
            .to_string();
        let md5 = body_md5(&body);

        // Alphabetical order is required by the Pusher signing spec.
        let mut params = [
            ("auth_key", self.config.key.clone()),
            ("auth_timestamp", timestamp),
            ("auth_version", AUTH_VERSION.to_string()),
            ("body_md5", md5),
        ];
        params.sort_by(|a, b| a.0.cmp(b.0));
        let query = params
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("&");

        let signature = hmac_sha256_hex(
            &self.config.secret,
            &string_to_sign(&self.config.events_path(), &query),
        )?;

        let url = format!("{endpoint}?{query}&auth_signature={signature}");
        Ok((url, body))
    }

    /// Build the client-side channel authorization for `socket_id`.
    ///
    /// `private` channels sign `{socket_id}:{channel}`; `presence` channels sign
    /// `{socket_id}:{channel}:{channel_data}` and echo `channel_data` back. The
    /// returned `auth` is `{key}:{hex_signature}`.
    ///
    /// # Errors
    ///
    /// [`BroadcastError::Transport`] if the HMAC context cannot be initialized.
    pub fn auth_signature(
        &self,
        socket_id: &str,
        wire_channel: &str,
        channel_data: Option<&str>,
    ) -> Result<ChannelAuth> {
        let to_sign = channel_auth_string(socket_id, wire_channel, channel_data);
        let signature = hmac_sha256_hex(&self.config.secret, &to_sign)?;
        Ok(ChannelAuth {
            auth: format!("{}:{signature}", self.config.key),
            channel_data: channel_data.map(str::to_string),
        })
    }
}

#[async_trait]
impl Broadcaster for PusherBroadcaster {
    /// The Pusher connection label.
    fn connection(&self) -> &'static str {
        "pusher"
    }

    /// Sign and POST the payload, mapping non-2xx responses to typed errors.
    async fn publish(&self, payload: BroadcastPayload) -> Result<()> {
        let (url, body) = self.signed_request(&payload)?;
        let (status, response_body) = self.transport.post_json(url, body).await?;
        if (200..300).contains(&status) {
            return Ok(());
        }
        let snippet = body_snippet(&response_body);
        let message = if status == 401 || status == 403 {
            format!("authentication failed (status {status}): {snippet}")
        } else {
            format!("unexpected status {status}: {snippet}")
        };
        Err(BroadcastError::Driver {
            connection: "pusher".to_string(),
            message,
        })
    }
}

/// Truncate a response body to a short, log-safe snippet.
fn body_snippet(body: &str) -> String {
    const LIMIT: usize = 256;
    let trimmed = body.trim();
    if trimmed.chars().count() <= LIMIT {
        trimmed.to_string()
    } else {
        let truncated: String = trimmed.chars().take(LIMIT).collect();
        format!("{truncated}…")
    }
}
