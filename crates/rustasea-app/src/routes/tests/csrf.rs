//! CSRF-gate tests — the mutating-route protection wired in [`super`].
//!
//! Positive: a same-origin `POST /login` carrying the expected token still
//! throttles, authenticates, sets the session cookie, and answers `303`.
//! Negative: cross-site writes (and a missing token) are rejected `403` and set
//! no cookie. Regression: ungated `GET` routes are untouched by the gate — the
//! guard matches an exact `(method, path)` allow-list, so it cannot leak.
//!
//! Split into a sibling module so `tests.rs` stays well under the 500-line cap.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderValue, Request, StatusCode};
use axum::Router;
use rustasea::auth::users::AuthUserRecord;
use rustasea::auth::verify::{Argon2Verifier, PasswordVerifier};
use rustasea::auth::{MemoryUserProvider, SessionGuard, SessionPolicy};
use rustasea::http::AppState;

use super::{app, call, get, method_request};
use crate::routes::helpers::{csrf_token, install_user_provider};
use crate::routes::{compile, table};

/// The expected CSRF token the compiled gate validates against.
fn token() -> String {
    csrf_token().to_string()
}

/// Add the token + a `same-origin` signal to a request (the passing case).
fn same_origin(mut request: Request<Body>) -> Request<Body> {
    request.headers_mut().insert(
        "x-csrf-token",
        HeaderValue::from_str(&token()).expect("token"),
    );
    request
        .headers_mut()
        .insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
    request
}

/// Add the token but a `cross-site` signal + an untrusted `Origin`.
fn cross_site(mut request: Request<Body>) -> Request<Body> {
    request.headers_mut().insert(
        "x-csrf-token",
        HeaderValue::from_str(&token()).expect("token"),
    );
    request
        .headers_mut()
        .insert("sec-fetch-site", HeaderValue::from_static("cross-site"));
    request.headers_mut().insert(
        "origin",
        HeaderValue::from_static("https://evil.example.com"),
    );
    request
}

/// A urlencoded `POST` request with a body.
fn post_form(uri: &str, body: &'static str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .expect("build")
}

/// A `PATCH` request with a urlencoded body.
fn patch_form(uri: &str, body: &'static str) -> Request<Body> {
    Request::builder()
        .method("PATCH")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .expect("build")
}

/// Build a router with a session guard and a seeded, login-capable provider.
fn app_with_seeded_user() -> Router {
    let provider = MemoryUserProvider::default();
    provider.seed(AuthUserRecord {
        id: "user-1".to_string(),
        email: "csrf-positive@example.com".to_string(),
        password_hash: Argon2Verifier::new()
            .hash("s3cr3t-pass")
            .expect("hash the seeded password"),
        email_verified_at: Some("2026-01-01T00:00:00Z".to_string()),
    });
    // Install the provider the handler resolves through `helpers::user_provider`.
    install_user_provider(Arc::new(provider));

    let guard = SessionGuard::new(SessionPolicy::default());
    compile(
        table(),
        Arc::new(AppState::new("testing", true).with_auth(Arc::new(guard))),
    )
}

/// Positive: a same-origin `POST /login` with valid credentials still runs the
/// full flow — throttle, credential check, session cookie, `303` → `/dashboard`.
#[tokio::test]
async fn same_origin_login_still_authenticates_and_sets_cookie() {
    let request = same_origin(post_form(
        "/login",
        "email=csrf-positive@example.com&password=s3cr3t-pass",
    ));
    let (status, headers, _) = call(app_with_seeded_user(), request).await;

    assert_eq!(
        status,
        StatusCode::SEE_OTHER,
        "same-origin login must succeed"
    );
    assert_eq!(
        headers.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some("/dashboard"),
        "success must redirect to the configured home"
    );
    assert!(
        headers.get(header::SET_COOKIE).is_some(),
        "a successful login must set the session cookie"
    );
}

/// Negative: a cross-site `POST /login` (untrusted origin) is rejected `403`
/// and never sets a cookie.
#[tokio::test]
async fn cross_site_login_is_rejected_and_sets_no_cookie() {
    let request = cross_site(post_form(
        "/login",
        "email=csrf-positive@example.com&password=s3cr3t-pass",
    ));
    let (status, headers, body) = call(app(), request).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "cross-site login must be 403"
    );
    assert!(
        headers.get(header::SET_COOKIE).is_none(),
        "a rejected login must not set a cookie"
    );
    assert!(
        body.contains("CsrfError"),
        "the 403 body must be the CSRF error envelope; body: {body}"
    );
}

/// Fail-closed: a state-changing `POST /login` with **no** CSRF token is
/// rejected `403` even when it claims `same-origin` (the crate checks the token
/// first).
#[tokio::test]
async fn login_without_token_fails_closed() {
    let mut request = post_form(
        "/login",
        "email=csrf-positive@example.com&password=s3cr3t-pass",
    );
    request
        .headers_mut()
        .insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
    let (status, _, _) = call(app(), request).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// Negative: a cross-site `PATCH /settings/profile` is rejected `403`.
#[tokio::test]
async fn cross_site_profile_patch_is_rejected() {
    let request = cross_site(patch_form("/settings/profile", "name=Ada&email=a@b.test"));
    let (status, _, _) = call(app(), request).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// Negative: a cross-site `POST /logout` is rejected `403`.
#[tokio::test]
async fn cross_site_logout_is_rejected() {
    let (status, _, _) = call(app(), cross_site(method_request("POST", "/logout"))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// Negative: the still-`501` POST stubs are covered by the gate too — a
/// cross-site request is rejected before the handler's `501`.
#[tokio::test]
async fn cross_site_stub_posts_are_rejected() {
    for uri in [
        "/register",
        "/confirm-password",
        "/email/verification-notification",
    ] {
        let (status, _, _) = call(app(), cross_site(method_request("POST", uri))).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "POST {uri}");
    }
}

/// Positive: a same-origin `PATCH /settings/profile` passes the gate and
/// reaches the handler (which redirects an unauthenticated caller to `/login`,
/// not `403`) — proving the gate allows the write through.
#[tokio::test]
async fn same_origin_profile_patch_passes_the_gate() {
    let request = same_origin(patch_form("/settings/profile", "name=Ada&email=a@b.test"));
    let (status, headers, _) = call(app(), request).await;
    assert_eq!(
        status,
        StatusCode::FOUND,
        "the handler (not the CSRF gate) must answer a same-origin write"
    );
    assert_eq!(
        headers.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some("/login")
    );
}

/// Regression: ungated `GET` routes are unaffected by the CSRF gate — the
/// allow-list matches only the mutating `(method, path)` pairs, so a safe GET
/// never needs a token and is never rejected (no sticky leak).
#[tokio::test]
async fn ungated_get_routes_are_unaffected_by_csrf() {
    for uri in ["/", "/health"] {
        let (status, _, _) = call(app(), get(uri)).await;
        assert_eq!(status, StatusCode::OK, "GET {uri} must be unaffected");
    }
}
