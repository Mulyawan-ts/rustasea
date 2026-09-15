//! Two-factor authentication (AUTH-016) tests.
//!
//! Drives the compiled router against a real [`SessionGuard`], a real
//! [`MemoryUserProvider`], a real in-memory [`MemoryTwoFactorStore`], and a
//! deterministic [`SecretCipher`] — no mock returning hardcoded codes. The
//! provider/store/cipher seams are process-wide, so every test serializes
//! through the shared [`PROVIDER_LOCK`] and resets the wiring on drop.
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
use rustasea::auth::verify::{Argon2Verifier, PasswordVerifier};
use rustasea::auth::{
    code_at, AuthUser, MemoryTwoFactorStore, MemoryUserProvider, SecretCipher, SessionGuard,
    SessionPolicy,
};
use rustasea::http::AppState;
use rustasea::validation::serde_json::{self, Value};

use super::settings_flows::PROVIDER_LOCK;
use super::{csrf_same_origin, method_request};
use crate::routes::auth::two_factor::wiring::{
    install_two_factor_cipher, install_two_factor_store, reset_two_factor_wiring,
};
use crate::routes::helpers::install_user_provider;
use crate::routes::{compile, table, SESSION_COOKIE_NAME};

/// The seeded password for `user-a`.
const USER_A_PASSWORD: &str = "secret-for-user-a";
/// The seeded email for `user-a`.
const USER_A_EMAIL: &str = "ada@example.com";

/// Install a verified `user-a` provider into the process-wide seam.
fn verified_provider() -> Arc<MemoryUserProvider> {
    let provider = MemoryUserProvider::default();
    provider.seed(AuthUserRecord {
        id: "user-a".to_string(),
        email: USER_A_EMAIL.to_string(),
        password_hash: Argon2Verifier::new()
            .hash(USER_A_PASSWORD)
            .expect("hash the seeded password"),
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
        password_hash: "unused-for-login-using-id".to_string(),
        email_verified_at: None,
        timezone: None,
    });
    Arc::new(
        SessionGuard::new(SessionPolicy::default())
            .with_lookup(lookup)
            .with_allow_login_using_id(true),
    )
}

/// Install the store + a deterministic cipher, returning the store handle.
fn install_store() -> Arc<MemoryTwoFactorStore> {
    let store = Arc::new(MemoryTwoFactorStore::default());
    install_two_factor_store(store.clone());
    install_two_factor_cipher(Some(SecretCipher::from_bytes([7u8; 32])));
    store
}

/// Build the served router with `guard` installed as the auth backend.
fn app_with_guard(guard: Arc<SessionGuard>) -> Router {
    compile(
        table(),
        Arc::new(AppState::new("testing", true).with_auth(guard)),
    )
}

/// Build the served router with no auth guard (management-endpoint tests).
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

/// An authenticated principal that never confirmed its password.
fn unconfirmed_principal() -> AuthUser {
    AuthUser::new("user-a", Some(USER_A_EMAIL.to_string()), "session")
        .with_email_verified_at(Some("2026-01-01T00:00:00Z".to_string()))
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

/// A urlencoded POST carrying the CSRF signal.
fn post(uri: &str, body: &str) -> Request<Body> {
    let mut request = method_request("POST", uri);
    request.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    *request.body_mut() = Body::from(body.to_string());
    csrf_same_origin(request)
}

/// A urlencoded POST carrying a session cookie + the CSRF signal.
fn post_with_cookie(uri: &str, body: &str, session_id: &str) -> Request<Body> {
    let mut request = post(uri, body);
    request.headers_mut().insert(
        header::COOKIE,
        HeaderValue::from_str(&format!("{SESSION_COOKIE_NAME}={session_id}"))
            .expect("valid cookie header"),
    );
    request
}

/// A plain GET carrying a session cookie.
fn get_with_cookie(uri: &str, session_id: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header(
            header::COOKIE,
            format!("{SESSION_COOKIE_NAME}={session_id}"),
        )
        .body(Body::empty())
        .expect("build")
}

/// Read the `Location` header, if present.
fn location(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// Read the value of the `name` cookie from a `Set-Cookie` header.
fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(header::SET_COOKIE)?.to_str().ok()?;
    let pair = raw.split(';').next()?;
    let (key, value) = pair.split_once('=')?;
    (key == name).then(|| value.to_string())
}

/// Read a string field from a JSON response body.
fn json_str(body: &str, field: &str) -> String {
    serde_json::from_str::<Value>(body)
        .expect("json body")
        .get(field)
        .and_then(Value::as_str)
        .expect("string field")
        .to_string()
}

/// Read a string-array field from a JSON response body.
fn json_array(body: &str, field: &str) -> Vec<String> {
    serde_json::from_str::<Value>(body)
        .expect("json body")
        .get(field)
        .and_then(Value::as_array)
        .expect("array field")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// Enable two-factor auth for `user-a` through the HTTP management surface.
async fn enable() -> (String, Vec<String>) {
    let router = app().layer(Extension(fresh_principal()));
    let (status, _, body) = call(router, post("/user/two-factor-authentication", "")).await;
    assert_eq!(status, StatusCode::OK, "enable: {body}");
    (
        json_str(&body, "secret"),
        json_array(&body, "recovery_codes"),
    )
}

/// Confirm two-factor auth for `user-a` with a fresh TOTP code.
async fn confirm(secret: &str) {
    let code = code_at(secret, now_secs()).expect("current code");
    let router = app().layer(Extension(fresh_principal()));
    let (status, _, body) = call(
        router,
        post(
            "/user/confirmed-two-factor-authentication",
            &format!("code={code}"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "confirm: {body}");
}

/// Start a challenge by logging in; returns the pending session id.
async fn start_challenge(guard: &Arc<SessionGuard>) -> String {
    let body = format!("email={USER_A_EMAIL}&password={USER_A_PASSWORD}");
    let (status, headers, response) =
        call(app_with_guard(guard.clone()), post("/login", &body)).await;
    assert_eq!(status, StatusCode::FOUND, "login: {response}");
    assert_eq!(location(&headers).as_deref(), Some("/two-factor-challenge"));
    cookie_value(&headers, SESSION_COOKIE_NAME).expect("pending session cookie")
}

/// Current UNIX time in seconds.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after the epoch")
        .as_secs()
}

/// RAII guard that resets the process-wide two-factor seams on drop.
struct Wiring;

impl Wiring {
    /// Reset the seams before the test and return the guard.
    fn install() -> Self {
        reset_two_factor_wiring();
        Self
    }
}

impl Drop for Wiring {
    fn drop(&mut self) {
        reset_two_factor_wiring();
    }
}

/// Positive: enable → confirm → interrupted login → valid code authenticates;
/// a recovery code also completes the challenge exactly once.
#[tokio::test]
async fn enable_confirm_challenge_and_recovery_lifecycle() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _wiring = Wiring::install();
    let store = install_store();
    let _provider = verified_provider();
    let guard = guard_with_user_a();

    let (secret, recovery_codes) = enable().await;
    assert_eq!(recovery_codes.len(), 10);
    confirm(&secret).await;
    assert!(store.get_sync("user-a").expect("record").is_confirmed());

    // The challenge form renders for an unauthenticated visitor.
    let router = app_with_guard(guard.clone());
    let (status, _, body) = call(router, method_request("GET", "/two-factor-challenge")).await;
    assert_eq!(status, StatusCode::OK, "challenge page: {body}");
    assert!(body.contains("/two-factor-challenge"));

    // The GET view returns the cached plaintext codes.
    let router = app().layer(Extension(fresh_principal()));
    let (status, _, body) = call(
        router,
        method_request("GET", "/user/two-factor-recovery-codes"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_array(&body, "recovery_codes").len(), 10);

    // A confirmed login is interrupted (the user is still a guest).
    let pending = start_challenge(&guard).await;
    let router = app_with_guard(guard.clone());
    let (status, _, _) = call(router, get_with_cookie("/dashboard", &pending)).await;
    assert_eq!(
        status,
        StatusCode::FOUND,
        "pending session must stay a guest"
    );

    // A valid code completes the login.
    let code = code_at(&secret, now_secs()).expect("challenge code");
    let router = app_with_guard(guard.clone());
    let (status, headers, _) = call(
        router,
        post_with_cookie("/two-factor-challenge", &format!("code={code}"), &pending),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location(&headers).as_deref(), Some("/dashboard"));
    let session = cookie_value(&headers, SESSION_COOKIE_NAME).expect("session cookie");
    assert_ne!(session, pending, "the session id must be rotated");

    let router = app_with_guard(guard.clone());
    let (status, _, body) = call(router, get_with_cookie("/dashboard", &session)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Dashboard"));

    // A recovery code works once, then is rejected.
    let recovery = &recovery_codes[0];
    let pending = start_challenge(&guard).await;
    let router = app_with_guard(guard.clone());
    let (status, _, _) = call(
        router,
        post_with_cookie(
            "/two-factor-challenge",
            &format!("recovery_code={recovery}"),
            &pending,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "first recovery use");

    let pending = start_challenge(&guard).await;
    let router = app_with_guard(guard.clone());
    let (status, _, body) = call(
        router,
        post_with_cookie(
            "/two-factor-challenge",
            &format!("recovery_code={recovery}"),
            &pending,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "reuse: {body}");
    assert!(body.contains("AuthError::InvalidTwoFactorCode"));
}

/// Negative: an invalid TOTP code is rejected.
#[tokio::test]
async fn invalid_totp_code_is_rejected() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _wiring = Wiring::install();
    let _store = install_store();
    let _provider = verified_provider();
    let guard = guard_with_user_a();
    let (secret, _codes) = enable().await;
    confirm(&secret).await;

    let pending = start_challenge(&guard).await;
    let router = app_with_guard(guard.clone());
    let (status, _, body) = call(
        router,
        post_with_cookie("/two-factor-challenge", "code=000000", &pending),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body.contains("AuthError::InvalidTwoFactorCode"));
}

/// Negative: the challenge is throttled after repeated failures.
#[tokio::test]
async fn challenge_is_throttled_after_repeated_failures() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _wiring = Wiring::install();
    let _store = install_store();
    let _provider = verified_provider();
    let guard = guard_with_user_a();
    let (secret, _codes) = enable().await;
    confirm(&secret).await;

    let pending = start_challenge(&guard).await;
    let mut denied = false;
    for _ in 0..6 {
        let router = app_with_guard(guard.clone());
        let (status, _, _) = call(
            router,
            post_with_cookie("/two-factor-challenge", "code=000000", &pending),
        )
        .await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            denied = true;
            break;
        }
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    assert!(denied, "the two-factor limiter must eventually deny");
}

/// Negative: enabling without a password confirmation is rejected by the gate.
#[tokio::test]
async fn enable_without_password_confirmation_is_rejected() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _wiring = Wiring::install();
    let _store = install_store();
    let router = app().layer(Extension(unconfirmed_principal()));
    let (status, headers, _) = call(router, post("/user/two-factor-authentication", "")).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(location(&headers).as_deref(), Some("/confirm-password"));
}

/// Negative: disabling without a password confirmation is rejected by the gate.
#[tokio::test]
async fn disable_without_password_confirmation_is_rejected() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _wiring = Wiring::install();
    let _store = install_store();
    let router = app().layer(Extension(unconfirmed_principal()));
    let (status, headers, _) = call(
        router,
        csrf_same_origin(method_request("DELETE", "/user/two-factor-authentication")),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(location(&headers).as_deref(), Some("/confirm-password"));
}

/// Positive: disabling wipes the stored secret and recovery hashes.
#[tokio::test]
async fn disable_wipes_the_stored_record() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _wiring = Wiring::install();
    let store = install_store();
    let _provider = verified_provider();
    let (secret, _codes) = enable().await;
    confirm(&secret).await;
    assert!(store.get_sync("user-a").is_some());

    let router = app().layer(Extension(fresh_principal()));
    let (status, _, body) = call(
        router,
        csrf_same_origin(method_request("DELETE", "/user/two-factor-authentication")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(store.get_sync("user-a").is_none(), "the record is wiped");
}
