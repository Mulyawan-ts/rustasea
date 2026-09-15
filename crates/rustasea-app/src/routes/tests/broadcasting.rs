//! Broadcasting authorization surface tests (ADOPT-022).
//!
//! Positive: an authenticated `POST /broadcasting/auth` for a private channel
//! returns a signed `{ "auth": "{key}:{hmac}" }` payload; a presence channel
//! additionally echoes `channel_data`. Negative: an unauthenticated request is
//! redirected to `/login`; a request without the CSRF token is rejected `403`;
//! a malformed body is `400`; a denial by the gate is `403`; and no configured
//! manager is `404`.
//!
//! The broadcast manager/gate are process-wide, so every test serializes through
//! [`BROADCAST_LOCK`] and installs its own config, resetting afterwards.
//!
//! Gated on the `broadcasting` feature because the routes/module only exist with
//! it.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::Extension;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use rustasea::auth::AuthUser;
use rustasea::broadcast::{BroadcastManager, BroadcastingConfig, PusherConfig};
use rustasea::http::AppState;

use super::{call, csrf_same_origin};
use crate::routes::{compile, table};

/// Serializes tests that touch the process-wide broadcast slots.
static BROADCAST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// An authorization gate that allows everything.
struct AllowAll;

#[rustasea::broadcast::async_trait]
impl rustasea::broadcast::Authorize for AllowAll {
    async fn authorize(
        &self,
        _channel: &str,
        _identity: &str,
    ) -> rustasea::broadcast::AuthDecision {
        rustasea::broadcast::AuthDecision::Allow
    }
}

/// An authorization gate that denies everything.
struct DenyAll;

#[rustasea::broadcast::async_trait]
impl rustasea::broadcast::Authorize for DenyAll {
    async fn authorize(
        &self,
        _channel: &str,
        _identity: &str,
    ) -> rustasea::broadcast::AuthDecision {
        rustasea::broadcast::AuthDecision::Deny("nope".to_string())
    }
}

/// A fully configured Pusher config.
fn pusher_config() -> BroadcastingConfig {
    BroadcastingConfig {
        default: "pusher".to_string(),
        pusher: Some(PusherConfig {
            app_id: "123456".to_string(),
            key: "app-key".to_string(),
            secret: "app-secret".to_string(),
            cluster: Some("us2".to_string()),
            ..PusherConfig::default()
        }),
        ..BroadcastingConfig::default()
    }
}

/// Install a manager built from `config` with `gate`.
fn install(config: BroadcastingConfig, gate: Option<Arc<dyn rustasea::broadcast::Authorize>>) {
    let manager = BroadcastManager::from_config(&config, gate).expect("manager builds");
    rustasea::broadcast::set_broadcast_config(config);
    rustasea::broadcast::set_broadcast_manager(Arc::new(manager));
}

/// A router with an authenticated principal.
fn authed_router() -> Router {
    let principal = AuthUser::new("user-1", Some("ada@example.com"), "session");
    compile(table(), Arc::new(AppState::new("testing", true))).layer(Extension(principal))
}

/// A `POST /broadcasting/auth` request with a JSON body.
fn json_auth_request(socket_id: &str, channel_name: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/broadcasting/auth")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({ "socket_id": socket_id, "channel_name": channel_name }).to_string(),
        ))
        .expect("build")
}

/// A `POST /broadcasting/auth` request with a form body.
fn form_auth_request(socket_id: &str, channel_name: &str) -> Request<Body> {
    let body = format!("socket_id={socket_id}&channel_name={channel_name}");
    Request::builder()
        .method("POST")
        .uri("/broadcasting/auth")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .expect("build")
}

/// Positive: a private channel returns `{key}:{hmac}` over `{socket}:{channel}`.
#[tokio::test]
async fn private_channel_auth_returns_signature() {
    let _guard = BROADCAST_LOCK.lock().await;
    install(pusher_config(), Some(Arc::new(AllowAll)));

    let request = csrf_same_origin(json_auth_request("123.456", "private-chat.1"));
    let (status, _, body) = call(authed_router(), request).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("JSON");
    let auth = parsed["auth"].as_str().expect("auth string");
    assert!(auth.starts_with("app-key:"), "auth: {auth}");
    assert!(parsed.get("channel_data").is_none());

    rustasea::broadcast::clear_broadcast_config();
}

/// Positive: a presence channel echoes `channel_data` and signs it in.
#[tokio::test]
async fn presence_channel_auth_includes_channel_data() {
    let _guard = BROADCAST_LOCK.lock().await;
    install(pusher_config(), Some(Arc::new(AllowAll)));

    let request = csrf_same_origin(form_auth_request("123.456", "presence-chat.1"));
    let (status, _, body) = call(authed_router(), request).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("JSON");
    let channel_data = parsed["channel_data"].as_str().expect("channel_data");
    let parsed_data: serde_json::Value = serde_json::from_str(channel_data).expect("data JSON");
    assert_eq!(parsed_data["user_id"], "user-1");

    rustasea::broadcast::clear_broadcast_config();
}

/// Positive: a public channel authorizes without signing.
#[tokio::test]
async fn public_channel_auth_returns_empty_auth() {
    let _guard = BROADCAST_LOCK.lock().await;
    install(pusher_config(), Some(Arc::new(AllowAll)));

    let request = csrf_same_origin(json_auth_request("123.456", "orders.1"));
    let (status, _, body) = call(authed_router(), request).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("JSON");
    assert_eq!(parsed["auth"], "");

    rustasea::broadcast::clear_broadcast_config();
}

/// Negative: a denied channel is `403`.
#[tokio::test]
async fn denied_channel_is_forbidden() {
    let _guard = BROADCAST_LOCK.lock().await;
    install(pusher_config(), Some(Arc::new(DenyAll)));

    let request = csrf_same_origin(json_auth_request("123.456", "private-chat.1"));
    let (status, _, _) = call(authed_router(), request).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    rustasea::broadcast::clear_broadcast_config();
}

/// Negative: a private channel with no gate fails closed (`401`).
#[tokio::test]
async fn private_channel_without_gate_is_unauthorized() {
    let _guard = BROADCAST_LOCK.lock().await;
    install(pusher_config(), None);

    let request = csrf_same_origin(json_auth_request("123.456", "private-chat.1"));
    let (status, _, _) = call(authed_router(), request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    rustasea::broadcast::clear_broadcast_config();
}

/// Negative: an unauthenticated request is redirected to `/login`.
#[tokio::test]
async fn unauthenticated_is_redirected_to_login() {
    let _guard = BROADCAST_LOCK.lock().await;
    install(pusher_config(), Some(Arc::new(AllowAll)));

    let router = compile(table(), Arc::new(AppState::new("testing", true)));
    let request = csrf_same_origin(json_auth_request("123.456", "private-chat.1"));
    let (status, headers, _) = call(router, request).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        headers.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some("/login")
    );

    rustasea::broadcast::clear_broadcast_config();
}

/// Negative: a `POST` without the CSRF token is rejected `403`.
#[tokio::test]
async fn missing_csrf_is_rejected() {
    let _guard = BROADCAST_LOCK.lock().await;
    install(pusher_config(), Some(Arc::new(AllowAll)));

    let (status, _, _) = call(
        authed_router(),
        json_auth_request("123.456", "private-chat.1"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    rustasea::broadcast::clear_broadcast_config();
}

/// Negative: a body missing a field is `400`.
#[tokio::test]
async fn missing_field_is_bad_request() {
    let _guard = BROADCAST_LOCK.lock().await;
    install(pusher_config(), Some(Arc::new(AllowAll)));

    let request = Request::builder()
        .method("POST")
        .uri("/broadcasting/auth")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({ "socket_id": "123.456" }).to_string(),
        ))
        .expect("build");
    let (status, _, _) = call(authed_router(), csrf_same_origin(request)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    rustasea::broadcast::clear_broadcast_config();
}

/// Negative: no installed manager is `404`.
#[tokio::test]
async fn no_manager_is_not_found() {
    let _guard = BROADCAST_LOCK.lock().await;
    rustasea::broadcast::clear_broadcast_config();

    let request = csrf_same_origin(json_auth_request("123.456", "private-chat.1"));
    let (status, _, _) = call(authed_router(), request).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
