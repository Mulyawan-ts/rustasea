//! Session-layer wiring tests (AUTH-006).
//!
//! These exercise the real session → `Extension<AuthUser>` projection: a
//! [`SessionGuard`] seeded with an in-memory user mints a session id via
//! `login_using_id`, and that id is replayed as the `rustasea-session` cookie.
//! The positive case proves a valid cookie reaches the gated handler and the
//! principal is observable downstream; the negative cases prove an absent or
//! forged cookie still redirects without panicking.
//!
//! Split into a sibling module so `tests.rs` stays under the 500-line limit.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use rustasea::auth::users::{AuthUserRecord, MemoryUserRegistry};
use rustasea::auth::{Guard, SessionGuard, SessionPolicy};
use rustasea::http::AppState;

use super::{call, get};
use crate::routes::{
    compile, resolve_principal, session_id_from_headers, table, SESSION_COOKIE_NAME,
};

/// Seed a guard with a single in-memory user and enable `login_using_id`.
///
/// `verified` controls whether the seeded `email_verified_at` is present, so a
/// test can drive either the `auth`-only or the `auth`+`verified` gate.
fn guard_with_user(verified: bool) -> Arc<SessionGuard> {
    let lookup = Arc::new(MemoryUserRegistry::default());
    lookup.seed(AuthUserRecord {
        id: "user-1".to_string(),
        email: "ada@example.com".to_string(),
        password_hash: "unused-for-login-using-id".to_string(),
        email_verified_at: if verified {
            Some("2026-01-01T00:00:00Z".to_string())
        } else {
            None
        },
    });
    let guard = SessionGuard::new(SessionPolicy::default())
        .with_lookup(lookup)
        .with_allow_login_using_id(true);
    Arc::new(guard)
}

/// Mint a session id for `user_id` on `guard` (the value a real login cookie
/// would carry).
async fn mint_session(guard: &SessionGuard, user_id: &str) -> String {
    guard
        .login_using_id(user_id)
        .await
        .expect("login_using_id mints a session")
        .access_token
}

/// Build the served router with `guard` installed as the auth backend.
fn app_with_guard(guard: Arc<SessionGuard>) -> Router {
    compile(
        table(),
        Arc::new(AppState::new("testing", true).with_auth(guard)),
    )
}

/// A plain GET request carrying the session cookie for `session_id`.
fn get_with_session(uri: &str, session_id: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header(
            header::COOKIE,
            format!("{SESSION_COOKIE_NAME}={session_id}"),
        )
        .body(Body::empty())
        .expect("build")
}

/// Positive: a request carrying a valid session cookie reaches the gated
/// `/dashboard` handler (200) instead of the unauthenticated 302, and the
/// projected principal is observable downstream (the greeting interpolates the
/// seeded email) — proving the middleware inserts `Extension<AuthUser>`.
#[tokio::test]
async fn valid_session_cookie_authenticates_gated_route() {
    let guard = guard_with_user(true);
    let session_id = mint_session(&guard, "user-1").await;

    let (status, _, body) = call(
        app_with_guard(guard),
        get_with_session("/dashboard", &session_id),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a valid session must reach the handler"
    );
    assert!(
        body.contains("Welcome back, ada@example.com"),
        "the projected principal must be observable downstream; body: {body}"
    );
}

/// Positive: the global session layer also projects the principal on an
/// **ungated** route (`/` renders the auth-aware nav), which is why the layer
/// is applied globally rather than as the sticky `auth` middleware id.
#[tokio::test]
async fn valid_session_cookie_is_visible_on_ungated_route() {
    let guard = guard_with_user(true);
    let session_id = mint_session(&guard, "user-1").await;

    let (status, _, body) = call(app_with_guard(guard), get_with_session("/", &session_id)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("/logout") && body.contains("Log out"),
        "the logged-in nav must render for a valid session; body: {body}"
    );
    assert!(
        !body.contains(">Register<"),
        "the anonymous nav must not render when authenticated; body: {body}"
    );
}

/// Negative: with **no** session cookie the gated `/dashboard` still answers
/// `302` → `/login` — the pre-existing gate behaviour is preserved.
#[tokio::test]
async fn absent_session_cookie_still_redirects_to_login() {
    let (status, headers, body) =
        call(app_with_guard(guard_with_user(true)), get("/dashboard")).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        headers.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some("/login")
    );
    assert!(!body.contains("Dashboard"));
}

/// Negative: a forged/garbage session id does not authenticate and does not
/// panic — the gate redirects exactly as for an absent cookie.
#[tokio::test]
async fn forged_session_cookie_does_not_authenticate() {
    let guard = guard_with_user(true);
    for forged in ["garbage", "", "not-a-valid-session-id"] {
        let (status, headers, _) = call(
            app_with_guard(Arc::clone(&guard)),
            get_with_session("/dashboard", forged),
        )
        .await;
        assert_eq!(status, StatusCode::FOUND, "forged id {forged:?}");
        assert_eq!(
            headers.get(header::LOCATION).and_then(|v| v.to_str().ok()),
            Some("/login"),
            "forged id {forged:?} must not authenticate"
        );
    }
}

/// Negative: a well-formed but unknown session id is rejected (the store holds
/// no such record), so the gate still redirects.
#[tokio::test]
async fn unknown_session_id_is_rejected() {
    let guard = guard_with_user(true);
    // A syntactically valid `Id` (16 bytes, URL-safe base64) that was never
    // minted into this guard's store.
    let unknown = "AAAAAAAAAAAAAAAAAAAAAA";
    let (status, headers, _) = call(
        app_with_guard(guard),
        get_with_session("/dashboard", unknown),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        headers.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some("/login")
    );
}

/// Negative: an **unverified** session authenticates but is stopped by the
/// `verified` gate and redirected to the verification notice — the session
/// projection does not bypass the second gate.
#[tokio::test]
async fn valid_but_unverified_session_redirects_to_verification_notice() {
    let guard = guard_with_user(false);
    let session_id = mint_session(&guard, "user-1").await;

    let (status, headers, body) = call(
        app_with_guard(guard),
        get_with_session("/dashboard", &session_id),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        headers.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some("/verify-email")
    );
    assert!(!body.contains("Dashboard"));
}

/// Fail-closed: with **no** auth backend installed, `resolve_principal` yields
/// `None` for any session id rather than panicking.
#[tokio::test]
async fn resolve_principal_is_none_without_guard() {
    assert_eq!(resolve_principal(None, "any-session-id").await, None);
}

/// The cookie reader returns the session value from a multi-cookie header and
/// ignores unrelated cookies.
#[test]
fn session_id_from_headers_extracts_the_named_cookie() {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        header::COOKIE,
        axum::http::HeaderValue::from_static("theme=dark; rustasea-session=abc123; locale=en"),
    );
    assert_eq!(session_id_from_headers(&headers).as_deref(), Some("abc123"));
}

/// The cookie reader fails closed for an absent header, an absent named cookie,
/// and a non-UTF-8 header.
#[test]
fn session_id_from_headers_fails_closed() {
    let empty = axum::http::HeaderMap::new();
    assert_eq!(session_id_from_headers(&empty), None);

    let mut unrelated = axum::http::HeaderMap::new();
    unrelated.insert(
        header::COOKIE,
        axum::http::HeaderValue::from_static("theme=dark"),
    );
    assert_eq!(session_id_from_headers(&unrelated), None);

    let mut invalid = axum::http::HeaderMap::new();
    invalid.insert(
        header::COOKIE,
        axum::http::HeaderValue::from_bytes(b"rustasea-session=\xff").expect("opaque bytes"),
    );
    assert_eq!(session_id_from_headers(&invalid), None);
}
