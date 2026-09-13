//! Settings profile-flow tests (FIX-AUTH-02, part a).
//!
//! These pin the behaviour of `PATCH /settings/profile` (`profile_update` in
//! [`crate::routes::settings`]) against a real [`MemoryUserProvider`] seeded
//! with two distinct users and genuine Argon2 password hashes — no mock that
//! returns hardcoded values.
//!
//! What is proven:
//!
//! * **Self-exclusion** — the rule string `unique:users,email,<current-user-id>`
//!   accepts the caller's *own* unchanged email (no `email` field error). This
//!   would fail if `ignore_id` were dropped, so it is a genuine regression pin.
//! * **Wiring** — another user's email is rejected with `422` and a field error
//!   on `email`, proving `unique` is actually evaluated rather than passing
//!   vacuously.
//! * **Re-verification** — changing the email while
//!   `[fortify].features.email_verification` is on clears `email_verified_at`;
//!   re-submitting the same email leaves it untouched.
//! * **Auth gate** — an unauthenticated write is `302` → `/login` (the handler
//!   runs behind the CSRF gate, which every mutating request must satisfy).
//! * **Fail-closed** — with [`DenyAllProvider`] installed the `unique` probe
//!   cannot resolve, so validation fails with the typed missing-context error
//!   instead of silently passing.
//! * **X-Forwarded-For trust** — [`crate::routes::auth::peer_ip`] honours the
//!   forwarded header only when the deployment declares trusted proxies (part b
//!   of FIX-AUTH-02); the password-flow tests live in the sibling
//!   [`super::settings_password`] module.
//!
//! The [`UserProvider`] seam is a process-wide cell, so the tests that install a
//! provider are serialized through [`PROVIDER_LOCK`] to keep them deterministic
//! under libtest's parallel runner.
//!
//! [`MemoryUserProvider`]: rustasea::auth::MemoryUserProvider
//! [`DenyAllProvider`]: rustasea::auth::DenyAllProvider
//! [`UserProvider`]: rustasea::auth::UserProvider

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, Extension};
use axum::http::{header, HeaderMap, HeaderValue, Request, StatusCode};
use rustasea::auth::users::AuthUserRecord;
use rustasea::auth::verify::{Argon2Verifier, PasswordVerifier};
use rustasea::auth::{AuthUser, DenyAllProvider, MemoryUserProvider};
use rustasea::validation::serde_json::{self, Value};

use super::{app, call, csrf_same_origin, method_request};
use crate::routes::auth::peer_ip;
use crate::routes::helpers::{fortify_config, install_user_provider};

/// The profile-update endpoint under test.
const PROFILE_URI: &str = "/settings/profile";

/// Verification timestamp shared by the seeded records.
const VERIFIED_AT: &str = "2026-01-01T00:00:00Z";

/// Serializes the provider-installing tests in this module.
///
/// The [`UserProvider`](rustasea::auth::UserProvider) seam is a process-wide
/// cell ([`crate::routes::helpers::user_provider`]), so two tests installing
/// different providers concurrently would race. A `tokio` mutex (not `std`) is
/// used because the guard is held across an `.await` and must stay `Send`; the
/// [`LazyLock`](std::sync::LazyLock) wrapper is needed because
/// `tokio::sync::Mutex::new` is not a `const fn`.
///
/// Shared with the sibling [`super::settings_password`] module so the two
/// modules' provider-installing tests stay mutually exclusive.
pub(super) static PROVIDER_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Build a profile `PATCH` request carrying a urlencoded body.
///
/// Reuses the shared [`method_request`] builder for the method + URI so the
/// request shape stays consistent with the other route tests.
fn profile_patch(body: &str) -> Request<Body> {
    let mut request = method_request("PATCH", PROFILE_URI);
    request.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    *request.body_mut() = Body::from(body.to_string());
    request
}

/// Seed one user record with a real Argon2 hash for `email`.
///
/// Shared with the sibling [`super::settings_password`] module so both suites
/// seed byte-identical records (the stored hash is `secret-for-<id>`).
pub(super) fn record(id: &str, email: &str) -> AuthUserRecord {
    AuthUserRecord {
        id: id.to_string(),
        email: email.to_string(),
        password_hash: Argon2Verifier::new()
            .hash(&format!("secret-for-{id}"))
            .expect("hash the seeded password"),
        email_verified_at: Some(VERIFIED_AT.to_string()),
    }
}

/// Seed two distinct users and install the provider into the shared seam.
///
/// Returns the concrete handle so a test can inspect the stored records after
/// the request (e.g. to assert `email_verified_at` was cleared, or that a
/// password hash rotated).
pub(super) fn seeded_provider() -> Arc<MemoryUserProvider> {
    let provider = MemoryUserProvider::default();
    provider.seed(record("user-a", "ada@example.com"));
    provider.seed(record("user-b", "grace@example.com"));
    let provider = Arc::new(provider);
    install_user_provider(provider.clone());
    provider
}

/// The authenticated principal for the first seeded user (`user-a`).
pub(super) fn user_a_principal() -> AuthUser {
    AuthUser::new("user-a", Some("ada@example.com"), "session")
        .with_email_verified_at(Some(VERIFIED_AT))
}

/// The `email` field errors parsed from a `422` validation body.
///
/// A success (`303`) response has an empty body, which parses as non-JSON and
/// yields an empty list — the caller pairs this with the status assertion.
fn email_errors(body: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    value
        .get("errors")
        .and_then(|errors| errors.get("email"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Positive: re-submitting the caller's **own** unchanged email is accepted.
///
/// The `unique:users,email,<id>` self-exclusion makes the probe answer
/// `Some(true)` when the found owner equals `ignore_id`. If `ignore_id` were
/// dropped the probe would report `Some(false)` and the response would carry an
/// `email` field error, so asserting the absence of that error genuinely pins
/// the exclusion.
#[tokio::test]
async fn own_unchanged_email_passes_unique_self_exclusion() {
    let _guard = PROVIDER_LOCK.lock().await;
    let provider = seeded_provider();

    let (status, _, body) = call(
        app().layer(Extension(user_a_principal())),
        csrf_same_origin(profile_patch("name=Ada&email=ada@example.com")),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::SEE_OTHER,
        "the caller's own email must be accepted; body: {body}"
    );
    assert!(
        email_errors(&body).is_empty(),
        "no `email` field error may be raised for the caller's own address; body: {body}"
    );
    // Self-exclusion must not evict or reassign the record.
    assert_eq!(
        provider.by_email("ada@example.com").map(|r| r.id),
        Some("user-a".to_string()),
        "the unchanged email must still belong to user-a"
    );
}

/// Negative: an email owned by **another** user is rejected `422` with a field
/// error on `email` — proving `unique` is wired and not passing vacuously.
#[tokio::test]
async fn another_users_email_is_rejected_by_unique() {
    let _guard = PROVIDER_LOCK.lock().await;
    let _provider = seeded_provider();

    let (status, _, body) = call(
        app().layer(Extension(user_a_principal())),
        csrf_same_origin(profile_patch("name=Ada&email=grace@example.com")),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "another user's email must be rejected; body: {body}"
    );
    assert!(
        !email_errors(&body).is_empty(),
        "a field error on `email` is required; body: {body}"
    );
}

/// Positive: changing the email while `email_verification` is enabled clears
/// the stored `email_verified_at`, forcing the new address to be re-verified.
#[tokio::test]
async fn email_change_clears_verification_when_enabled() {
    let _guard = PROVIDER_LOCK.lock().await;
    assert!(
        fortify_config().features.email_verification,
        "this test pins the email-verification-on branch"
    );
    let provider = seeded_provider();

    let (status, _, _) = call(
        app().layer(Extension(user_a_principal())),
        csrf_same_origin(profile_patch("name=Ada&email=ada-new@example.com")),
    )
    .await;

    assert_eq!(status, StatusCode::SEE_OTHER);
    let moved = provider
        .by_email("ada-new@example.com")
        .expect("the email change must persist");
    assert_eq!(moved.id, "user-a");
    assert!(
        moved.email_verified_at.is_none(),
        "a changed email must be re-verified (email_verified_at cleared)"
    );
}

/// Positive: re-submitting the **same** email leaves `email_verified_at`
/// untouched — the clearing step is guarded by a real change comparison.
#[tokio::test]
async fn unchanged_email_leaves_verification_untouched() {
    let _guard = PROVIDER_LOCK.lock().await;
    let provider = seeded_provider();

    let (status, _, _) = call(
        app().layer(Extension(user_a_principal())),
        csrf_same_origin(profile_patch("name=Ada&email=ada@example.com")),
    )
    .await;

    assert_eq!(status, StatusCode::SEE_OTHER);
    let record = provider.by_id("user-a").expect("user-a is still present");
    assert_eq!(
        record.email_verified_at.as_deref(),
        Some(VERIFIED_AT),
        "an unchanged email must keep its verification timestamp"
    );
}

/// Negative: an unauthenticated write is `302` → `/login`. The request still
/// carries the CSRF token + same-origin signal so the gate lets it reach the
/// handler (otherwise the assertion would see the gate's `403`, not the auth
/// redirect).
#[tokio::test]
async fn unauthenticated_profile_patch_redirects_to_login() {
    let _guard = PROVIDER_LOCK.lock().await;
    let _provider = seeded_provider();

    let (status, headers, body) = call(
        app(),
        csrf_same_origin(profile_patch("name=Ada&email=ada@example.com")),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::FOUND,
        "an unauthenticated write must redirect to login; body: {body}"
    );
    assert_eq!(
        headers.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some("/login")
    );
}

/// Fail-closed: with [`DenyAllProvider`] installed the `unique` probe cannot be
/// resolved, so validation returns the typed missing-context failure (`422`)
/// rather than silently accepting the address.
#[tokio::test]
async fn provider_unavailable_fails_unique_closed() {
    let _guard = PROVIDER_LOCK.lock().await;
    install_user_provider(Arc::new(DenyAllProvider));

    let (status, _, body) = call(
        app().layer(Extension(user_a_principal())),
        csrf_same_origin(profile_patch("name=Ada&email=ada@example.com")),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "an unavailable provider must fail closed; body: {body}"
    );
    let errors = email_errors(&body);
    assert!(
        !errors.is_empty(),
        "a typed error on `email` is required; body: {body}"
    );
    assert!(
        errors
            .iter()
            .any(|message| message.contains("could not be validated")),
        "must be the typed missing-context failure, not a silent pass; errors: {errors:?}"
    );
}

// ---------------------------------------------------------------------------
// X-Forwarded-For trust (FIX-AUTH-02, part b)
// ---------------------------------------------------------------------------

/// Build a header map carrying an optional `X-Forwarded-For` value.
fn headers_with_xff(value: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(value) = value {
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_str(value).expect("valid header value"),
        );
    }
    headers
}

/// Build a `ConnectInfo<SocketAddr>` for `ip` on a fixed port.
fn connect_info(ip: &str) -> Option<ConnectInfo<SocketAddr>> {
    Some(ConnectInfo(
        format!("{ip}:54321").parse().expect("valid socket addr"),
    ))
}

/// The trust decision is delegated to the crate primitive, so with **no**
/// trusted proxies a client-supplied `X-Forwarded-For` is ignored and the
/// connection peer wins — a client behind no proxy cannot spoof its bucket.
#[test]
fn peer_ip_ignores_xff_without_trusted_proxies() {
    let headers = headers_with_xff(Some("9.9.9.9"));
    assert_eq!(
        peer_ip(&headers, connect_info("1.2.3.4"), &[]),
        "1.2.3.4",
        "an untrusted X-Forwarded-For must be ignored in favour of the peer"
    );
}

/// With a trusted proxy declared, the first hop of `X-Forwarded-For` is
/// honoured — the forwarded client address is what the throttle buckets on.
#[test]
fn peer_ip_honours_xff_when_proxies_trusted() {
    let headers = headers_with_xff(Some("9.9.9.9"));
    let trusted = vec!["10.0.0.0/8".to_string()];
    assert_eq!(
        peer_ip(&headers, connect_info("10.0.0.1"), &trusted),
        "9.9.9.9",
        "a trusted proxy's X-Forwarded-For must win over the proxy peer"
    );
}

/// Fallback ordering: when a trusted proxy sends an `X-Forwarded-For` list the
/// first (client-most) hop is used; when the header is absent the connection
/// peer is used instead.
#[test]
fn peer_ip_fallback_ordering() {
    let trusted = vec!["10.0.0.0/8".to_string()];

    let with_list = headers_with_xff(Some("9.9.9.9, 10.0.0.1, 10.0.0.2"));
    assert_eq!(
        peer_ip(&with_list, connect_info("10.0.0.1"), &trusted),
        "9.9.9.9",
        "the first hop of the forwarded list is the client identity"
    );

    let absent = headers_with_xff(None);
    assert_eq!(
        peer_ip(&absent, connect_info("1.2.3.4"), &trusted),
        "1.2.3.4",
        "an absent X-Forwarded-For must fall back to the connection peer"
    );
}

/// Fail-closed fallback: with neither connection info nor `X-Forwarded-For`
/// the throttle still gets a finite bucket (the `UNKNOWN_PEER` constant) rather
/// than an unbounded/absent key.
#[test]
fn peer_ip_falls_back_to_constant_when_nothing_available() {
    let headers = headers_with_xff(None);
    assert_eq!(
        peer_ip(&headers, None, &[]),
        "0.0.0.0",
        "no peer and no header must yield the finite fallback bucket"
    );
}
