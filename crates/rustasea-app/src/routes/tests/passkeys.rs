//! Passkey / WebAuthn (AUTH-017) route tests.
//!
//! Drives the compiled router against a real [`SessionGuard`], a real
//! [`MemoryUserProvider`], and real in-memory passkey/challenge stores. The
//! cryptographic ceremonies are covered exhaustively in `rustasea-auth`
//! (`passkeys::tests`, which sign real P-256 assertions); here we exercise the
//! HTTP surface: discovery, feature gating, CSRF, listing, ceremony start, and
//! the typed error paths. The provider/store seams are process-wide, so every
//! test serializes through the shared [`PROVIDER_LOCK`] and resets the wiring on
//! drop.
//!
//! [`SessionGuard`]: rustasea::auth::SessionGuard
//! [`MemoryUserProvider`]: rustasea::auth::MemoryUserProvider
//! [`PROVIDER_LOCK`]: super::settings_flows::PROVIDER_LOCK

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::Extension;
use axum::http::{header, HeaderMap, HeaderValue, Request, StatusCode};
use axum::Router;
use rustasea::auth::users::{AuthUserRecord, MemoryUserRegistry};
use rustasea::auth::{
    AuthUser, MemoryChallengeStore, MemoryUserProvider, SessionGuard, SessionPolicy,
};
use rustasea::http::AppState;
use rustasea::validation::serde_json::{self, Value};

use super::settings_flows::PROVIDER_LOCK;
use super::{csrf_same_origin, method_request};
use crate::routes::auth::passkeys::set_passkeys_disabled;
use crate::routes::auth::passkeys::wiring::{install_challenge_store, reset_passkey_wiring};
use crate::routes::helpers::install_user_provider;
use crate::routes::{compile, table, SESSION_COOKIE_NAME};

/// The seeded email for `user-a`.
const USER_A_EMAIL: &str = "ada@example.com";

/// Install a verified `user-a` provider into the process-wide seam.
fn install_provider() -> Arc<MemoryUserProvider> {
    let provider = MemoryUserProvider::default();
    provider.seed(AuthUserRecord {
        id: "user-a".to_string(),
        email: USER_A_EMAIL.to_string(),
        password_hash: "unused-for-passkeys".to_string(),
        email_verified_at: Some("2026-01-01T00:00:00Z".to_string()),
        timezone: None,
    });
    let provider = Arc::new(provider);
    install_user_provider(provider.clone());
    provider
}

/// Seed a session guard whose lookup resolves the same `user-a` identity.
fn guard_with_user_a() -> Arc<SessionGuard> {
    let lookup = Arc::new(MemoryUserRegistry::default());
    lookup.seed(AuthUserRecord {
        id: "user-a".to_string(),
        email: USER_A_EMAIL.to_string(),
        password_hash: "unused".to_string(),
        email_verified_at: None,
        timezone: None,
    });
    Arc::new(
        SessionGuard::new(SessionPolicy::default())
            .with_lookup(lookup)
            .with_allow_login_using_id(true),
    )
}

/// Build the served router with `guard` installed as the auth backend.
fn app_with_guard(guard: Arc<SessionGuard>) -> Router {
    compile(
        table(),
        Arc::new(AppState::new("testing", true).with_auth(guard)),
    )
}

/// Build the served router with no auth guard.
fn app() -> Router {
    compile(table(), Arc::new(AppState::new("testing", true)))
}

/// An authenticated principal with a fresh password confirmation.
fn fresh_principal() -> AuthUser {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after the epoch")
        .as_secs();
    AuthUser::new("user-a", Some(USER_A_EMAIL.to_string()), "session")
        .with_email_verified_at(Some("2026-01-01T00:00:00Z".to_string()))
        .with_password_confirmed_at(Some(now.to_string()))
}

/// Send one request and return status, headers, and body text.
async fn call(router: Router, request: Request<Body>) -> (StatusCode, HeaderMap, String) {
    let response = router.oneshot(request).await.expect("router dispatch");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

use tower::ServiceExt;

/// A JSON POST carrying the CSRF signal.
fn json_post(uri: &str, body: &str) -> Request<Body> {
    let mut request = method_request("POST", uri);
    request.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    *request.body_mut() = Body::from(body.to_string());
    csrf_same_origin(request)
}

/// A JSON POST with no CSRF signal (for the gate test).
fn json_post_no_csrf(uri: &str, body: &str) -> Request<Body> {
    let mut request = method_request("POST", uri);
    request.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    *request.body_mut() = Body::from(body.to_string());
    request
}

/// A DELETE carrying the CSRF signal.
fn delete(uri: &str) -> Request<Body> {
    csrf_same_origin(method_request("DELETE", uri))
}

/// Read the value of the `name` cookie from a `Set-Cookie` header.
fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(header::SET_COOKIE)?.to_str().ok()?;
    let pair = raw.split(';').next()?;
    let (key, value) = pair.split_once('=')?;
    (key == name).then(|| value.to_string())
}

/// RAII guard that resets the process-wide passkey seams on drop.
struct Wiring;

impl Wiring {
    /// Reset the seams before the test and return the guard.
    fn install() -> Self {
        reset_passkey_wiring();
        set_passkeys_disabled(false);
        Self
    }
}

impl Drop for Wiring {
    fn drop(&mut self) {
        reset_passkey_wiring();
        set_passkeys_disabled(false);
    }
}

/// Positive: the discovery endpoint advertises the settings security route.
#[tokio::test]
async fn well_known_advertises_security_route() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _wiring = Wiring::install();
    let (status, _, body) = call(
        app(),
        method_request("GET", "/.well-known/passkey-endpoints"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let json: Value = serde_json::from_str(&body).expect("json body");
    assert_eq!(json["enroll"], "/settings/security");
    assert_eq!(json["manage"], "/settings/security");
}

/// Negative: a disabled feature answers `404` on every passkey route.
#[tokio::test]
async fn disabled_feature_returns_404() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _wiring = Wiring::install();
    set_passkeys_disabled(true);

    let (status, _, _) = call(
        app(),
        method_request("GET", "/.well-known/passkey-endpoints"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _, _) = call(app(), json_post("/passkeys/login", "{}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let router = app().layer(Extension(fresh_principal()));
    let (status, _, _) = call(router, method_request("GET", "/user/passkeys")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Positive: an authenticated user with no credentials lists an empty array.
#[tokio::test]
async fn list_returns_empty_array() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _wiring = Wiring::install();
    let router = app().layer(Extension(fresh_principal()));
    let (status, _, body) = call(router, method_request("GET", "/user/passkeys")).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body.trim(), "[]");
}

/// Positive: starting a registration returns creation options with a challenge.
#[tokio::test]
async fn manage_begin_returns_creation_options() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _wiring = Wiring::install();
    let _provider = install_provider();
    let router = app().layer(Extension(fresh_principal()));
    let (status, _, body) = call(router, json_post("/user/passkeys", "{}")).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let json: Value = serde_json::from_str(&body).expect("json body");
    assert!(json["challenge"].as_str().is_some(), "body: {body}");
    assert!(json["rp"].is_object(), "body: {body}");
}

/// Negative: a completed registration with a bad attestation is `422`.
#[tokio::test]
async fn manage_finish_with_bad_attestation_is_422() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _wiring = Wiring::install();
    let _provider = install_provider();
    let router = app().layer(Extension(fresh_principal()));
    // Begin first so a challenge exists, then present an unverifiable response.
    let (status, _, _) = call(router, json_post("/user/passkeys", "{}")).await;
    assert_eq!(status, StatusCode::OK);

    let router = app().layer(Extension(fresh_principal()));
    let body = r#"{"id":"abc","response":{"clientDataJSON":"AA","attestationObject":"AA"}}"#;
    let (status, _, response) = call(router, json_post("/user/passkeys", body)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {response}");
    assert!(response.contains("AuthError::Passkey"), "body: {response}");
}

/// Positive: deleting a credential is idempotent and reports success.
#[tokio::test]
async fn delete_reports_success() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _wiring = Wiring::install();
    let router = app().layer(Extension(fresh_principal()));
    let (status, _, body) = call(router, delete("/user/passkeys/missing")).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(body.contains(r#""deleted":true"#), "body: {body}");
}

/// Positive: starting a login mints a session cookie + request options.
#[tokio::test]
async fn login_begin_returns_options_and_cookie() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _wiring = Wiring::install();
    let _provider = install_provider();
    let guard = guard_with_user_a();
    let (status, headers, body) =
        call(app_with_guard(guard), json_post("/passkeys/login", "{}")).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let json: Value = serde_json::from_str(&body).expect("json body");
    assert!(json["challenge"].as_str().is_some(), "body: {body}");
    assert!(
        cookie_value(&headers, SESSION_COOKIE_NAME).is_some(),
        "expected a session cookie"
    );
}

/// Negative: completing a login without a session cookie redirects to `/login`.
#[tokio::test]
async fn login_finish_without_session_redirects() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _wiring = Wiring::install();
    let _provider = install_provider();
    let guard = guard_with_user_a();
    let body = r#"{"id":"abc","response":{"clientDataJSON":"AA","authenticatorData":"AA","signature":"AA"}}"#;
    let (status, headers, _) =
        call(app_with_guard(guard), json_post("/passkeys/login", body)).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        headers
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some("/login")
    );
}

/// Negative: a mutating passkey route without the CSRF token is rejected.
#[tokio::test]
async fn mutating_route_requires_csrf() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _wiring = Wiring::install();
    let router = app().layer(Extension(fresh_principal()));
    let (status, _, _) = call(router, json_post_no_csrf("/user/passkeys", "{}")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// Positive: starting a registration stores a single-use pending challenge.
#[tokio::test]
async fn begin_stores_a_pending_challenge() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _wiring = Wiring::install();
    let _provider = install_provider();
    let store = Arc::new(MemoryChallengeStore::default());
    install_challenge_store(store.clone());
    let router = app().layer(Extension(fresh_principal()));
    let (status, _, _) = call(router, json_post("/user/passkeys", "{}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(store.get_sync("passkey:register:user-a").is_some());
}
