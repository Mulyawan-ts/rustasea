//! Route table tests — positive behaviour, named lookup, and real gating.
//!
//! Drives the compiled router through a single `tower::ServiceExt::oneshot`
//! dispatch (the app is a binary crate, so these live as crate-internal unit
//! tests rather than an integration suite).

use std::sync::Arc;

use axum::body::Body;
use axum::extract::Extension;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use rustasea::auth::AuthUser;
use rustasea::http::AppState;
use tower::ServiceExt;

use super::{compile, table};

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

/// Positive: an authenticated principal (`Extension<AuthUser>`) passes the
/// `auth` gate and reaches the dashboard handler — proving the gate is a real
/// identity check, not a blanket redirect.
#[tokio::test]
async fn authenticated_dashboard_reaches_handler() {
    let principal = AuthUser::new("user-1", Some("ada@example.com"), "session");
    let router = app().layer(Extension(principal));

    let (status, _, body) = call(router, get("/dashboard")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Dashboard"));
}

/// Unimplemented POST flows answer `501` with a clear message — no panic and
/// no faked success.
#[tokio::test]
async fn unimplemented_post_flows_return_501() {
    let (status, _, body) = call(app(), method_request("POST", "/login")).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert!(body.contains("not implemented"), "body: {body}");
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

/// Positive: an authenticated `/dashboard` renders the full app layout —
/// head partial, footer, and the `user`-aware greeting.
#[tokio::test]
async fn authenticated_dashboard_renders_the_layout() {
    let principal = AuthUser::new("user-1", Some("ada@example.com"), "session");
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
