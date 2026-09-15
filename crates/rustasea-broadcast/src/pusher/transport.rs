//! Pusher HTTP transport seam (ADOPT-022).
//!
//! The [`PusherTransport`] trait abstracts the outbound HTTP POST so the driver
//! is testable hermetically: the production [`ReqwestTransport`] uses
//! `reqwest::Client` with a configured timeout, while tests install a mock that
//! records the URL and body and returns a canned status.
//!
//! The trait returns `(status, body)` rather than a decoded value so the driver
//! owns the status-code policy (401/403 → auth error, other non-2xx → driver
//! error).

use async_trait::async_trait;
use std::time::Duration;

use crate::error::{BroadcastError, Result};

/// One outbound POST to the Pusher API.
#[async_trait]
pub trait PusherTransport: Send + Sync + 'static {
    /// POST `body` to `url`, returning the HTTP status and response body.
    ///
    /// # Errors
    ///
    /// [`BroadcastError::Driver`] on a transport-level failure (DNS, connection,
    /// timeout).
    async fn post_json(&self, url: String, body: String) -> Result<(u16, String)>;
}

/// `reqwest`-backed transport with a per-request timeout.
pub(crate) struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    /// Build a transport with the given request timeout.
    ///
    /// A client-build failure falls back to `reqwest::Client::new()` so a
    /// misconfigured TLS backend cannot panic the process.
    pub(crate) fn new(timeout: Duration) -> Self {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { client }
    }
}

#[async_trait]
impl PusherTransport for ReqwestTransport {
    /// POST the signed body and return `(status, body_text)`.
    async fn post_json(&self, url: String, body: String) -> Result<(u16, String)> {
        let response = self
            .client
            .post(&url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|error| BroadcastError::Driver {
                connection: "pusher".to_string(),
                message: format!("transport error: {error}"),
            })?;
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        Ok((status, text))
    }
}
