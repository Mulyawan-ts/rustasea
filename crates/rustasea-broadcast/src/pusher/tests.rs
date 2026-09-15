//! Pusher driver tests: signing, endpoints, transport failures, and client auth
//! (ADOPT-022). All tests use a recording mock transport, so they are hermetic.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::signing::{body_md5, hmac_sha256_hex, string_to_sign};
use super::{PusherBroadcaster, PusherConfig, PusherTransport};
use crate::broadcaster::{BroadcastPayload, Broadcaster};
use crate::error::{BroadcastError, Result};

/// A mock transport that records the last `(url, body)` and returns a canned
/// status.
struct MockTransport {
    status: u16,
    body: String,
    recorded: Mutex<Vec<(String, String)>>,
}

impl MockTransport {
    fn returning(status: u16, body: &str) -> Self {
        Self {
            status,
            body: body.to_string(),
            recorded: Mutex::new(Vec::new()),
        }
    }

    fn last(&self) -> Option<(String, String)> {
        self.recorded.lock().ok()?.last().cloned()
    }
}

#[async_trait]
impl PusherTransport for MockTransport {
    async fn post_json(&self, url: String, body: String) -> Result<(u16, String)> {
        if let Ok(mut guard) = self.recorded.lock() {
            guard.push((url, body));
        }
        Ok((self.status, self.body.clone()))
    }
}

/// A fully configured cluster-backed Pusher config.
fn cluster_config() -> PusherConfig {
    PusherConfig {
        app_id: "123456".to_string(),
        key: "app-key".to_string(),
        secret: "app-secret".to_string(),
        cluster: Some("us2".to_string()),
        ..PusherConfig::default()
    }
}

/// Parse `key=value` pairs from a query string into a map.
fn query_map(url: &str) -> std::collections::HashMap<String, String> {
    let query = url.split_once('?').map(|(_, q)| q).unwrap_or("");
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Positive: the request targets the cluster endpoint and carries every
/// required signed query parameter, with a signature that recomputes.
#[tokio::test]
async fn publish_signs_and_targets_cluster_endpoint() {
    let transport = Arc::new(MockTransport::returning(200, "{}"));
    let broadcaster = PusherBroadcaster::with_transport(cluster_config(), transport.clone());

    let payload = BroadcastPayload::new(
        "UserCreated",
        "private-chat.1",
        serde_json::json!({ "name": "Ada" }),
    );
    broadcaster.publish(payload).await.expect("publish");

    let (url, body) = transport.last().expect("request recorded");
    assert!(
        url.starts_with("https://api-us2.pusher.com/apps/123456/events?"),
        "url: {url}"
    );

    let params = query_map(&url);
    for key in [
        "auth_key",
        "auth_timestamp",
        "auth_version",
        "body_md5",
        "auth_signature",
    ] {
        assert!(params.contains_key(key), "missing {key} in {url}");
    }
    assert_eq!(params["auth_key"], "app-key");
    assert_eq!(params["auth_version"], "1.0");
    assert_eq!(params["body_md5"], body_md5(&body));

    // Recompute the signature independently.
    let unsigned = url.split("&auth_signature=").next().unwrap();
    let query = unsigned.split_once('?').unwrap().1;
    let expected =
        hmac_sha256_hex("app-secret", &string_to_sign("/apps/123456/events", query)).unwrap();
    assert_eq!(params["auth_signature"], expected);

    // The body shape: name, channels (with prefix), data as a JSON string.
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("body JSON");
    assert_eq!(parsed["name"], "UserCreated");
    assert_eq!(parsed["channels"][0], "private-chat.1");
    assert!(parsed["data"].is_string(), "data must be a JSON string");
    let data: serde_json::Value =
        serde_json::from_str(parsed["data"].as_str().unwrap()).expect("data JSON");
    assert_eq!(data["name"], "Ada");
}

/// A self-hosted `host` overrides the cluster endpoint with scheme/port.
#[tokio::test]
async fn host_config_targets_self_hosted_endpoint() {
    let transport = Arc::new(MockTransport::returning(200, "{}"));
    let config = PusherConfig {
        app_id: "42".to_string(),
        key: "k".to_string(),
        secret: "s".to_string(),
        host: Some("127.0.0.1".to_string()),
        ..PusherConfig::default()
    };
    let broadcaster = PusherBroadcaster::with_transport(config, transport.clone());
    broadcaster
        .publish(BroadcastPayload::new(
            "Ping",
            "orders.1",
            serde_json::json!({}),
        ))
        .await
        .expect("publish");

    let (url, _) = transport.last().expect("request recorded");
    assert!(
        url.starts_with("http://127.0.0.1:6001/apps/42/events?"),
        "url: {url}"
    );
}

/// Negative: a 401 surfaces a typed `Driver` auth error, never a silent drop.
#[tokio::test]
async fn unauthorized_status_is_driver_error() {
    let transport = Arc::new(MockTransport::returning(401, "{\"error\":\"bad key\"}"));
    let broadcaster = PusherBroadcaster::with_transport(cluster_config(), transport);
    let err = broadcaster
        .publish(BroadcastPayload::new(
            "Ping",
            "orders.1",
            serde_json::json!({}),
        ))
        .await
        .unwrap_err();
    match err {
        BroadcastError::Driver {
            ref connection,
            ref message,
        } => {
            assert_eq!(connection, "pusher");
            assert!(message.contains("401"), "message: {message}");
            assert!(message.contains("authentication"), "message: {message}");
        }
        other => panic!("expected Driver error, got {other:?}"),
    }
}

/// Negative: a 500 is a non-auth `Driver` error.
#[tokio::test]
async fn server_error_is_driver_error() {
    let transport = Arc::new(MockTransport::returning(500, "boom"));
    let broadcaster = PusherBroadcaster::with_transport(cluster_config(), transport);
    let err = broadcaster
        .publish(BroadcastPayload::new(
            "Ping",
            "orders.1",
            serde_json::json!({}),
        ))
        .await
        .unwrap_err();
    match err {
        BroadcastError::Driver { ref message, .. } => {
            assert!(message.contains("500"), "message: {message}");
        }
        other => panic!("expected Driver error, got {other:?}"),
    }
}

/// Negative: a config with no endpoint is not complete and cannot sign.
#[test]
fn config_without_endpoint_is_incomplete() {
    let config = PusherConfig {
        app_id: "1".to_string(),
        key: "k".to_string(),
        secret: "s".to_string(),
        cluster: None,
        host: None,
        ..PusherConfig::default()
    };
    assert!(!config.is_complete());
    assert!(matches!(
        config.endpoint(),
        Err(BroadcastError::NotConfigured { .. })
    ));
}

/// A config missing the secret is incomplete.
#[test]
fn config_without_secret_is_incomplete() {
    let config = PusherConfig {
        app_id: "1".to_string(),
        key: "k".to_string(),
        secret: String::new(),
        cluster: Some("us2".to_string()),
        ..PusherConfig::default()
    };
    assert!(!config.is_complete());
}

/// Private channel auth: `{key}:{hmac(socket:channel)}` recomputed
/// independently.
#[test]
fn private_channel_auth_signature() {
    let broadcaster = PusherBroadcaster::new(cluster_config());
    let auth = broadcaster
        .auth_signature("123.456", "private-chat.1", None)
        .expect("auth");
    let expected_sig = hmac_sha256_hex("app-secret", "123.456:private-chat.1").unwrap();
    assert_eq!(auth.auth, format!("app-key:{expected_sig}"));
    assert_eq!(auth.channel_data, None);
}

/// Presence channel auth: includes `channel_data` in the signed string and
/// echoes it back.
#[test]
fn presence_channel_auth_signature() {
    let broadcaster = PusherBroadcaster::new(cluster_config());
    let channel_data = "{\"user_id\":\"1\",\"user_info\":{\"name\":\"Ada\"}}";
    let auth = broadcaster
        .auth_signature("123.456", "presence-chat.1", Some(channel_data))
        .expect("auth");
    let expected_sig = hmac_sha256_hex(
        "app-secret",
        &format!("123.456:presence-chat.1:{channel_data}"),
    )
    .unwrap();
    assert_eq!(auth.auth, format!("app-key:{expected_sig}"));
    assert_eq!(auth.channel_data.as_deref(), Some(channel_data));
}
