//! Route table tests — positive behaviour, named lookup, and real gating.
//!
//! Drives the compiled router through a single `tower::ServiceExt::oneshot`
//! dispatch (the app is a binary crate, so these live as crate-internal unit
//! tests rather than an integration suite).

use std::sync::Arc;

use axum::body::Body;
use axum::extract::Extension;
use axum::http::{header, HeaderValue, Request, StatusCode};
use axum::Router;
use rustasea::auth::AuthUser;
use rustasea::http::AppState;
use tower::ServiceExt;

use super::{compile, table};

/// Session-layer wiring tests (AUTH-006), split out to keep this file small.
mod session;

/// CSRF-gate tests (FIX-AUTH-01), split out to keep this file small.
mod csrf;

mod settings_flows;

/// Settings password-flow tests (FIX-AUTH-02, part b), split into a sibling
/// module so `settings_flows.rs` stays under the 500-line cap.
mod settings_password;

/// Build the served router with a throwaway state.
fn app() -> Router {
    compile(table(), Arc::new(AppState::new("testing", true)))
}

/// Send one request and return status, headers, and body text.
async fn call(
    router: Router,
    request: Request<Body>,
) -> (StatusCode, axum::http::HeaderMap, String) {
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

/// A plain GET request for `uri`.
fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("build")
}

/// A plain request with an explicit method for `uri`.
fn method_request(method: &str, uri: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .expect("build")
}

/// Add the expected CSRF token + a `same-origin` signal to a request so the
/// gate lets a mutating route reach its handler.
pub(super) fn csrf_same_origin(mut request: Request<Body>) -> Request<Body> {
    let token = super::helpers::csrf_token();
    request
        .headers_mut()
        .insert("x-csrf-token", HeaderValue::from_str(token).expect("token"));
    request
        .headers_mut()
        .insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
    request
}

/// Positive: `/` and `/welcome` serve the welcome page unchanged.
#[tokio::test]
async fn root_and_welcome_serve_the_welcome_page() {
    for uri in ["/", "/welcome"] {
        let (status, _, body) = call(app(), get(uri)).await;
        assert_eq!(status, StatusCode::OK, "GET {uri}");
        assert!(
            body.contains("RustaSea"),
            "GET {uri} must render the welcome page"
        );
    }
}

/// Positive: `/health` returns the JSON liveness probe unchanged.
#[tokio::test]
async fn health_returns_json_probe() {
    let (status, _, body) = call(app(), get("/health")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(r#""status":"ok""#), "body: {body}");
    assert!(body.contains(r#""service":"rustasea-app""#), "body: {body}");
    assert!(body.contains(r#""env":"testing""#), "body: {body}");
}

/// Positive: `/settings` redirects (302) to `/settings/profile`.
#[tokio::test]
async fn settings_redirects_to_profile() {
    let (status, headers, _) = call(app(), get("/settings")).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        headers.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some("/settings/profile")
    );
}

/// Positive: the named route `profile.edit` resolves to `/settings/profile`.
#[test]
fn named_profile_edit_resolves() {
    let table = table();
    assert_eq!(
        table.url("profile.edit", &[]).as_deref(),
        Some("/settings/profile")
    );
    assert_eq!(table.url("home", &[]).as_deref(), Some("/"));
    assert_eq!(table.url("dashboard", &[]).as_deref(), Some("/dashboard"));
    assert_eq!(
        table.url("password.confirm", &[]).as_deref(),
        Some("/confirm-password")
    );
}

/// Negative: an unauthenticated `/dashboard` never reaches its handler —
/// the `auth` gate answers `302` → `/login` (not the dashboard HTML).
#[tokio::test]
async fn unauthenticated_dashboard_is_gated() {
    let (status, headers, body) = call(app(), get("/dashboard")).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        headers.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some("/login")
    );
    assert!(
        !body.contains("Dashboard"),
        "the dashboard handler must not run when unauthenticated"
    );
}

/// Negative: `/settings/security` is gated by `password.confirm`.
#[tokio::test]
async fn security_settings_is_gated() {
    let (status, headers, _) = call(app(), get("/settings/security")).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        headers.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some("/login")
    );
}

/// Positive: an authenticated, **verified** principal (`Extension<AuthUser>`)
/// passes the `auth` + `verified` gates and reaches the dashboard handler —
/// proving the gates are real identity checks, not a blanket redirect.
#[tokio::test]
async fn authenticated_dashboard_reaches_handler() {
    let principal = AuthUser::new("user-1", Some("ada@example.com"), "session")
        .with_email_verified_at(Some("2026-01-01T00:00:00Z"));
    let router = app().layer(Extension(principal));

    let (status, _, body) = call(router, get("/dashboard")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Dashboard"));
}

/// Negative: an authenticated but **unverified** principal is rejected by the
/// `verified` gate and redirected to the verification notice — the placeholder
/// degradation to a plain auth check is gone.
#[tokio::test]
async fn unverified_dashboard_redirects_to_verification_notice() {
    let principal = AuthUser::new("user-1", Some("ada@example.com"), "session");
    assert!(principal.email_verified_at.is_none());
    let router = app().layer(Extension(principal));

    let (status, headers, body) = call(router, get("/dashboard")).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        headers.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some("/verify-email")
    );
    assert!(
        !body.contains("Dashboard"),
        "the dashboard handler must not run for an unverified principal"
    );
}

/// The POST flows that are genuinely still stubbed answer `501` with a clear
/// message — no panic and no faked success. `/login` and `/logout` are now
/// implemented, so they are covered separately below. Each request carries the
/// CSRF token + a same-origin signal so the gate lets it reach the handler.
#[tokio::test]
async fn remaining_unimplemented_post_flows_return_501() {
    for uri in [
        "/register",
        "/confirm-password",
        "/email/verification-notification",
    ] {
        let (status, _, body) = call(app(), csrf_same_origin(method_request("POST", uri))).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "POST {uri}");
        assert!(body.contains("not implemented"), "POST {uri} body: {body}");
    }
}

/// `/login` and `/logout` are no longer stubbed: with no session guard wired
/// into the throwaway state the login handler fails closed (`500`) and logout
/// still redirects (`303`), but neither answers the old `501`. Each request
/// carries the CSRF token + a same-origin signal so the gate lets it through.
#[tokio::test]
async fn login_and_logout_are_no_longer_stubbed() {
    for uri in ["/login", "/logout"] {
        let (status, _, _) = call(app(), csrf_same_origin(method_request("POST", uri))).await;
        assert_ne!(status, StatusCode::NOT_IMPLEMENTED, "POST {uri}");
    }
}

/// Positive: the static asset route serves `resources/css/app.css` with a CSS
/// content type, so the stylesheet the app shell links is reachable over HTTP.
#[tokio::test]
async fn asset_route_serves_the_stylesheet() {
    let (status, headers, body) = call(app(), get("/assets/css/app.css")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("text/css")
    );
    assert!(
        body.contains("--color-accent"),
        "the served stylesheet must be the real design-token entry"
    );
}

/// Negative: path-traversal attempts against the asset route must not escape
/// the `resources/` root. `tower-http`'s `ServeDir` owns all path handling
/// (percent-decoding plus a `..`/backslash rejection) and answers `404`, so no
/// workspace file — here the root `Cargo.toml` — is ever returned.
#[tokio::test]
async fn asset_route_rejects_path_traversal() {
    for uri in [
        "/assets/../Cargo.toml",
        "/assets/..%2FCargo.toml",
        "/assets/%2e%2e/Cargo.toml",
        "/assets/../resources/css/app.css",
    ] {
        let (status, _, body) = call(app(), get(uri)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "GET {uri}");
        assert!(
            !body.contains("[package]"),
            "GET {uri} must not leak workspace file contents"
        );
    }
}

/// Positive: `/` renders through the shared layout chrome — the
/// `partials/head.html` stylesheet link and the auth-aware nav — rather than a
/// bare string literal.
#[tokio::test]
async fn root_renders_layout_chrome() {
    let (status, _, body) = call(app(), get("/")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(r#"href="/assets/css/app.css""#),
        "the head partial must link the stylesheet"
    );
    assert!(body.contains("<nav"), "the layout must render its nav");
}

/// Positive: an authenticated, verified `/dashboard` renders the full app
/// layout — head partial, footer, and the `user`-aware greeting.
#[tokio::test]
async fn authenticated_dashboard_renders_the_layout() {
    let principal = AuthUser::new("user-1", Some("ada@example.com"), "session")
        .with_email_verified_at(Some("2026-01-01T00:00:00Z"));
    let router = app().layer(Extension(principal));

    let (status, _, body) = call(router, get("/dashboard")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(r#"href="/assets/css/app.css""#),
        "the head partial must link the stylesheet"
    );
    assert!(
        body.contains("<footer"),
        "the layout must render its footer"
    );
    assert!(
        body.contains("Welcome back, ada@example.com"),
        "the greeting must interpolate the `user` context"
    );
}

/// Negative: an authenticated principal that never confirmed its password is
/// rejected by the `password.confirm` gate on `/settings/security` and sent to
/// the confirm-password screen.
#[tokio::test]
async fn unconfirmed_security_settings_redirects_to_confirm_password() {
    let principal = AuthUser::new("user-1", Some("ada@example.com"), "session");
    assert!(principal.password_confirmed_at.is_none());
    let router = app().layer(Extension(principal));

    let (status, headers, body) = call(router, get("/settings/security")).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        headers.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some("/confirm-password")
    );
    assert!(
        !body.contains("Security settings"),
        "the security handler must not run without a fresh confirmation"
    );
}

/// Positive: a principal with a **fresh** password confirmation passes the
/// `password.confirm` gate and reaches the security handler.
#[tokio::test]
async fn freshly_confirmed_security_settings_reaches_handler() {
    // A timestamp of "now" is always inside the window, whatever the timeout.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after the epoch")
        .as_secs();
    let principal = AuthUser::new("user-1", Some("ada@example.com"), "session")
        .with_password_confirmed_at(Some(now.to_string()));
    let router = app().layer(Extension(principal));

    let (status, _, body) = call(router, get("/settings/security")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Security settings"));
}

/// Negative: an authenticated principal whose confirmation is older than the
/// window is rejected by the `password.confirm` gate (stale confirmation).
#[tokio::test]
async fn stale_confirmation_is_rejected_by_password_gate() {
    // The epoch itself is far older than any sane timeout.
    let principal = AuthUser::new("user-1", Some("ada@example.com"), "session")
        .with_password_confirmed_at(Some("0"));
    let router = app().layer(Extension(principal));

    let (status, headers, _) = call(router, get("/settings/security")).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        headers.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some("/confirm-password")
    );
}

/// Positive: the `verification.notice` route the unverified gate redirects to
/// is actually served (the redirect target is not a dangling 404).
#[tokio::test]
async fn verification_notice_route_is_served() {
    let (status, _, body) = call(app(), get("/verify-email")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Verify your email"));
}
