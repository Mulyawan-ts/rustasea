//! Authentication-log emission tests (ADOPT-003).
//!
//! These drive the compiled router against a real [`SessionGuard`] and a real
//! [`MemoryUserProvider`] while an in-memory SQLite
//! [`AuthenticationLogLogger`] is installed into the process-wide slot. They
//! prove the HTTP layer emits the four event kinds:
//!
//! * a successful login writes a row with the resolved user id, IP, user agent,
//!   and `successful = true`;
//! * a wrong password writes `successful = false`;
//! * repeated failed attempts eventually record a `lockout` row (and answer
//!   `429`);
//! * a logout fills `logout_at` on the open login row.
//!
//! The provider seam and the authentication-logger slot are both process-wide,
//! so every test serializes through the shared [`PROVIDER_LOCK`] and clears the
//! logger slot on completion (including on panic, via [`LoggerGuard`]).
//!
//! [`SessionGuard`]: rustasea::auth::SessionGuard
//! [`MemoryUserProvider`]: rustasea::auth::MemoryUserProvider
//! [`AuthenticationLogLogger`]: rustasea_authlog::AuthenticationLogLogger
//! [`PROVIDER_LOCK`]: super::settings_flows::PROVIDER_LOCK

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderValue, Request, StatusCode};
use axum::Router;
use rustasea::auth::users::AuthUserRecord;
use rustasea::auth::verify::{Argon2Verifier, PasswordVerifier};
use rustasea::auth::{MemoryUserProvider, SessionGuard, SessionPolicy};
use rustasea::http::AppState;
use rustasea::orm::DbPool;
use rustasea_authlog::{AuthLogEventKind, AuthenticationLogLogger};

use super::{call, csrf_same_origin, method_request};
use crate::routes::helpers::install_user_provider;
use crate::routes::{compile, table, SESSION_COOKIE_NAME};

/// The seeded account email.
const EMAIL: &str = "ada@example.com";

/// The seeded account password.
const PASSWORD: &str = "s3cr3t-pass";

/// Create the `authentication_log` table with SQLite-appropriate column types.
async fn create_table(pool: &DbPool) {
    pool.execute_script(
        "CREATE TABLE authentication_log (
            id BLOB PRIMARY KEY,
            user_id TEXT,
            email TEXT,
            guard_name TEXT,
            event TEXT NOT NULL,
            ip_address TEXT,
            user_agent TEXT,
            successful INTEGER NOT NULL,
            login_at TEXT,
            logout_at TEXT,
            cleared_by_user_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
    )
    .await
    .expect("create authentication_log");
}

/// Build an in-memory pool with the `authentication_log` table applied.
async fn pool_with_table() -> DbPool {
    let pool = DbPool::connect("sqlite::memory:").await.expect("connect");
    create_table(&pool).await;
    pool
}

/// Install the logger into the process-wide slot and clear it on drop.
struct LoggerGuard;

impl LoggerGuard {
    /// Install `logger` for the lifetime of the guard.
    fn install(logger: Arc<AuthenticationLogLogger>) -> Self {
        rustasea_authlog::install(logger);
        Self
    }
}

impl Drop for LoggerGuard {
    fn drop(&mut self) {
        rustasea_authlog::clear();
    }
}

/// Seed a provider with one verified, login-capable user and install it.
fn install_provider() {
    let provider = MemoryUserProvider::default();
    provider.seed(AuthUserRecord {
        id: "user-1".to_string(),
        email: EMAIL.to_string(),
        password_hash: Argon2Verifier::new()
            .hash(PASSWORD)
            .expect("hash the seeded password"),
        email_verified_at: Some("2026-01-01T00:00:00Z".to_string()),
    });
    install_user_provider(Arc::new(provider));
}

/// Build the served router with a fresh [`SessionGuard`] installed.
fn app_with_guard() -> Router {
    app_with_shared_guard(Arc::new(SessionGuard::new(SessionPolicy::default())))
}

/// Build the served router over an existing guard (so its store is shared).
fn app_with_shared_guard(guard: Arc<SessionGuard>) -> Router {
    compile(
        table(),
        Arc::new(AppState::new("testing", true).with_auth(guard)),
    )
}

/// A urlencoded `POST` with a body, carrying the CSRF token + same-origin.
fn login_post(email: &str, password: &str) -> Request<Body> {
    let body = format!("email={email}&password={password}");
    let mut request = Request::builder()
        .method("POST")
        .uri("/login")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .expect("build");
    request = csrf_same_origin(request);
    request
}

/// Extract the `rustasea-session` cookie value from a response's `Set-Cookie`.
fn session_cookie(headers: &axum::http::HeaderMap) -> Option<String> {
    let raw = headers.get(header::SET_COOKIE)?.to_str().ok()?;
    let pair = raw.split(';').next()?;
    let (name, value) = pair.split_once('=')?;
    (name == SESSION_COOKIE_NAME).then(|| value.to_string())
}

/// Positive: a successful login writes a row with ip + user agent.
#[tokio::test]
async fn successful_login_records_row() {
    let _lock = super::settings_flows::PROVIDER_LOCK.lock().await;
    install_provider();
    let pool = pool_with_table().await;
    let logger = Arc::new(AuthenticationLogLogger::new(pool.clone()));
    let _guard = LoggerGuard::install(logger.clone());

    let mut request = login_post(EMAIL, PASSWORD);
    request
        .headers_mut()
        .insert(header::USER_AGENT, HeaderValue::from_static("test-agent"));
    let (status, _, _) = call(app_with_guard(), request).await;

    assert_eq!(status, StatusCode::SEE_OTHER, "login must succeed");
    let rows = logger.latest(10).await.expect("query");
    assert_eq!(rows.len(), 1, "exactly one row");
    let row = &rows[0];
    assert_eq!(row.event, "login_succeeded");
    assert_eq!(row.user_id.as_deref(), Some("user-1"));
    assert_eq!(row.ip_address.as_deref(), Some("0.0.0.0"));
    assert_eq!(row.user_agent.as_deref(), Some("test-agent"));
    assert!(row.successful);
    assert!(row.login_at.is_some());
    pool.close().await;
}

/// Negative: a wrong password writes `successful = false`.
#[tokio::test]
async fn failed_login_records_unsuccessful_row() {
    let _lock = super::settings_flows::PROVIDER_LOCK.lock().await;
    install_provider();
    let pool = pool_with_table().await;
    let logger = Arc::new(AuthenticationLogLogger::new(pool.clone()));
    let _guard = LoggerGuard::install(logger.clone());

    let (status, _, _) = call(app_with_guard(), login_post(EMAIL, "wrong-password")).await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let rows = logger
        .for_event(AuthLogEventKind::LoginFailed)
        .await
        .expect("query");
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].successful);
    assert_eq!(rows[0].event, "login_failed");
    pool.close().await;
}

/// Lockout: repeated failed attempts record a `lockout` row and answer `429`.
#[tokio::test]
async fn repeated_failures_record_lockout() {
    let _lock = super::settings_flows::PROVIDER_LOCK.lock().await;
    install_provider();
    let pool = pool_with_table().await;
    let logger = Arc::new(AuthenticationLogLogger::new(pool.clone()));
    let _guard = LoggerGuard::install(logger.clone());

    // The `login` limiter allows 5 attempts/minute per `username|ip`; the 6th is
    // denied. A unique email keeps this test's bucket clear of other tests.
    let email = "lockout-subject@example.com";
    let mut last_status = StatusCode::OK;
    for _ in 0..6 {
        let (status, _, _) = call(app_with_guard(), login_post(email, "wrong-password")).await;
        last_status = status;
    }

    assert_eq!(
        last_status,
        StatusCode::TOO_MANY_REQUESTS,
        "the 6th attempt must be throttled"
    );
    let lockouts = logger
        .for_event(AuthLogEventKind::Lockout)
        .await
        .expect("query");
    assert!(
        !lockouts.is_empty(),
        "a lockout row must be recorded for the blocked attempt"
    );
    assert!(!lockouts[0].successful);
    pool.close().await;
}

/// Positive: a logout fills `logout_at` on the open login row.
#[tokio::test]
async fn logout_fills_open_row() {
    let _lock = super::settings_flows::PROVIDER_LOCK.lock().await;
    install_provider();
    let pool = pool_with_table().await;
    let logger = Arc::new(AuthenticationLogLogger::new(pool.clone()));
    let _guard = LoggerGuard::install(logger.clone());

    // Log in to mint a session cookie and write the open login row.
    let guard = Arc::new(SessionGuard::new(SessionPolicy::default()));
    let (status, headers, _) = call(
        app_with_shared_guard(guard.clone()),
        login_post(EMAIL, PASSWORD),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let cookie = session_cookie(&headers).expect("login must set the session cookie");

    // Log out with that cookie, reusing the same guard so the session store is
    // the one the login wrote into.
    let mut request = method_request("POST", "/logout");
    request.headers_mut().insert(
        header::COOKIE,
        HeaderValue::from_str(&format!("{SESSION_COOKIE_NAME}={cookie}")).expect("cookie"),
    );
    let (status, _, _) = call(app_with_shared_guard(guard), csrf_same_origin(request)).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "logout must redirect");

    let rows = logger.for_user("user-1").await.expect("query");
    assert_eq!(rows.len(), 1, "logout updates the open row, not a new one");
    assert!(
        rows[0].logout_at.is_some(),
        "logout must fill `logout_at` on the open row"
    );
    pool.close().await;
}
