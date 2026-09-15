//! Settings password-flow tests (FIX-AUTH-02, part b).
//!
//! These pin the behaviour of `PUT /settings/password` (`password_update` in
//! [`crate::routes::settings`]) against a real [`MemoryUserProvider`] seeded
//! with genuine Argon2 hashes — no mock that returns hardcoded values. The
//! seeding helpers ([`seeded_provider`], [`user_a_principal`]) and the shared
//! [`PROVIDER_LOCK`] live in the sibling [`super::settings_flows`] module so the
//! two-user Argon2 setup is defined exactly once.
//!
//! What is proven:
//!
//! * **Rotation** — a correct current password plus a policy-satisfying new
//!   password answers `303` and the stored hash changes: the new password
//!   verifies and the old one no longer does.
//! * **Current-password check** — a wrong current password is rejected `422`
//!   and the stored hash is left untouched.
//! * **Policy** — a new password that is too short and missing a symbol is
//!   rejected `422` with the individual requirement violations.
//! * **Confirmation** — a mismatched `password_confirmation` is rejected `422`
//!   by the `confirmed` rule.
//! * **Auth gate** — an unauthenticated write is `302` → `/login`.
//! * **Session survival** — after a successful change the *same* session cookie
//!   still authenticates: the handler neither rotates nor invalidates the
//!   session (kit parity), and this test pins that deliberate decision so a
//!   future change to it is a conscious one.
//!
//! [`MemoryUserProvider`]: rustasea::auth::MemoryUserProvider

use std::sync::Arc;

use axum::body::Body;
use axum::extract::Extension;
use axum::http::{header, HeaderValue, Request, StatusCode};
use axum::Router;
use rustasea::auth::users::{AuthUserRecord, MemoryUserRegistry};
use rustasea::auth::verify::{Argon2Verifier, PasswordVerifier};
use rustasea::auth::{Guard, MemoryUserProvider, SessionGuard, SessionPolicy};
use rustasea::http::AppState;
use rustasea::validation::serde_json::{self, Value};

use super::settings_flows::{seeded_provider, user_a_principal, PROVIDER_LOCK};
use super::{app, call, csrf_same_origin, method_request};
use crate::routes::{compile, table, SESSION_COOKIE_NAME};

/// The password-update endpoint under test.
const PASSWORD_URI: &str = "/settings/password";

/// The seeded password for `user-a` (`record` hashes `secret-for-<id>`).
const OLD_PASSWORD: &str = "secret-for-user-a";

/// A password that satisfies [`PasswordPolicy::production`]: ≥ 12 chars with
/// mixed case, a letter, a number, and a symbol.
///
/// [`PasswordPolicy::production`]: rustasea::validation::PasswordPolicy::production
const NEW_PASSWORD: &str = "BrandNewPassw0rd!";

/// Build a password `PUT` request carrying a urlencoded body.
fn password_put(body: &str) -> Request<Body> {
    let mut request = method_request("PUT", PASSWORD_URI);
    request.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    *request.body_mut() = Body::from(body.to_string());
    request
}

/// Build the urlencoded body for a password change.
///
/// The values used in these tests contain only urlencoded-safe characters
/// (`!` is a sub-delimiter and is passed through verbatim by the hand-rolled
/// [`parse_form`](crate::routes::helpers::parse_form)), so no percent-encoding
/// is required.
fn form(current: &str, new: &str, confirmation: &str) -> String {
    format!("current_password={current}&password={new}&password_confirmation={confirmation}")
}

/// The stored Argon2 hash for `id`.
fn stored_hash(provider: &MemoryUserProvider, id: &str) -> String {
    provider
        .by_id(id)
        .expect("the seeded user is present")
        .password_hash
}

/// The messages under `field` in a `422` validation body.
fn field_errors(body: &str, field: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    value
        .get("errors")
        .and_then(|errors| errors.get(field))
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

/// Positive: a correct current password plus a policy-satisfying new password
/// answers `303` and the stored hash rotates — the new password verifies and
/// the old one no longer does.
#[tokio::test]
async fn correct_current_password_rotates_the_stored_hash() {
    let _lock = PROVIDER_LOCK.lock().await;
    let provider = seeded_provider();
    let verifier = Argon2Verifier::new();
    let before = stored_hash(&provider, "user-a");
    assert!(
        verifier.verify(&before, OLD_PASSWORD),
        "seed sanity: the old password must verify against the seeded hash"
    );

    let (status, _, body) = call(
        app().layer(Extension(user_a_principal())),
        csrf_same_origin(password_put(&form(
            OLD_PASSWORD,
            NEW_PASSWORD,
            NEW_PASSWORD,
        ))),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::SEE_OTHER,
        "a valid password change must succeed; body: {body}"
    );
    let after = stored_hash(&provider, "user-a");
    assert_ne!(
        before, after,
        "the stored hash must change after a password update"
    );
    assert!(
        verifier.verify(&after, NEW_PASSWORD),
        "the new password must verify against the new hash"
    );
    assert!(
        !verifier.verify(&after, OLD_PASSWORD),
        "the old password must no longer verify after the change"
    );
}

/// Negative: a wrong current password is rejected `422` and the stored hash is
/// left unchanged.
#[tokio::test]
async fn wrong_current_password_is_rejected_and_leaves_hash_unchanged() {
    let _lock = PROVIDER_LOCK.lock().await;
    let provider = seeded_provider();
    let before = stored_hash(&provider, "user-a");

    let (status, _, body) = call(
        app().layer(Extension(user_a_principal())),
        csrf_same_origin(password_put(&form(
            "not-the-current-password",
            NEW_PASSWORD,
            NEW_PASSWORD,
        ))),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a wrong current password must be rejected; body: {body}"
    );
    assert!(
        !field_errors(&body, "current_password").is_empty(),
        "a field error on `current_password` is required; body: {body}"
    );
    assert_eq!(
        stored_hash(&provider, "user-a"),
        before,
        "a rejected change must not touch the stored hash"
    );
}

/// Negative: a policy-violating new password (too short **and** missing a
/// symbol) is rejected `422` with the individual requirement violations, and
/// the stored hash is untouched.
#[tokio::test]
async fn policy_violating_new_password_is_rejected() {
    let _lock = PROVIDER_LOCK.lock().await;
    let provider = seeded_provider();
    let before = stored_hash(&provider, "user-a");

    // 9 chars (below the 12 minimum) with no symbol.
    let weak = "shortpass";
    let (status, _, body) = call(
        app().layer(Extension(user_a_principal())),
        csrf_same_origin(password_put(&form(OLD_PASSWORD, weak, weak))),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a weak new password must be rejected; body: {body}"
    );
    let errors = field_errors(&body, "password");
    assert!(
        !errors.is_empty(),
        "field errors on `password` are required; body: {body}"
    );
    assert!(
        errors.iter().any(|message| message.contains("at least")),
        "the length violation must be reported; errors: {errors:?}"
    );
    assert!(
        errors.iter().any(|message| message.contains("symbol")),
        "the missing-symbol violation must be reported; errors: {errors:?}"
    );
    assert_eq!(
        stored_hash(&provider, "user-a"),
        before,
        "a rejected change must not touch the stored hash"
    );
}

/// Negative: a mismatched `password_confirmation` is rejected `422` by the
/// `confirmed` rule.
#[tokio::test]
async fn mismatched_confirmation_is_rejected() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _provider = seeded_provider();

    let (status, _, body) = call(
        app().layer(Extension(user_a_principal())),
        csrf_same_origin(password_put(&form(
            OLD_PASSWORD,
            NEW_PASSWORD,
            "DifferentPassw0rd!",
        ))),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a mismatched confirmation must be rejected; body: {body}"
    );
    let errors = field_errors(&body, "password");
    assert!(
        errors
            .iter()
            .any(|message| message.contains("confirmation")),
        "the `confirmed` rule must fire; errors: {errors:?}"
    );
}

/// Negative: an unauthenticated write is `302` → `/login`. The request still
/// carries the CSRF token + same-origin signal so the gate lets it reach the
/// handler (otherwise the assertion would see the gate's `403`, not the auth
/// redirect).
#[tokio::test]
async fn unauthenticated_password_put_redirects_to_login() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _provider = seeded_provider();

    let (status, headers, body) = call(
        app(),
        csrf_same_origin(password_put(&form(
            OLD_PASSWORD,
            NEW_PASSWORD,
            NEW_PASSWORD,
        ))),
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

/// Seed a session guard whose lookup resolves the same `user-a` identity the
/// provider holds, so the session middleware projects the principal the
/// password handler reads.
fn guard_with_user_a() -> Arc<SessionGuard> {
    let lookup = Arc::new(MemoryUserRegistry::default());
    lookup.seed(AuthUserRecord {
        id: "user-a".to_string(),
        email: "ada@example.com".to_string(),
        // Never used: this guard only mints/parses session ids via
        // `login_using_id`, so the credential hash is irrelevant here.
        password_hash: "unused-for-login-using-id".to_string(),
        email_verified_at: Some("2026-01-01T00:00:00Z".to_string()),
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

/// A password `PUT` carrying the session cookie + the CSRF signal.
fn put_with_session(body: &str, session_id: &str) -> Request<Body> {
    let mut request = password_put(body);
    request.headers_mut().insert(
        header::COOKIE,
        HeaderValue::from_str(&format!("{SESSION_COOKIE_NAME}={session_id}"))
            .expect("valid cookie header"),
    );
    csrf_same_origin(request)
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

/// Positive: after a successful password change the **same** session cookie
/// still authenticates — the handler neither rotates nor invalidates the
/// session (kit parity). This pins the documented "session stays valid"
/// decision so a future change to it is deliberate, not accidental.
#[tokio::test]
async fn session_stays_valid_after_a_password_change() {
    let _lock = PROVIDER_LOCK.lock().await;
    // The password handler resolves the provider cell; the session guard
    // resolves the principal. Both must see the same `user-a`.
    let _provider = seeded_provider();
    let guard = guard_with_user_a();
    let session_id = guard
        .login_using_id("user-a")
        .await
        .expect("mint a session for user-a")
        .access_token;

    // Sanity: the cookie authenticates before the change.
    let (before, _, _) = call(
        app_with_guard(Arc::clone(&guard)),
        get_with_session("/dashboard", &session_id),
    )
    .await;
    assert_eq!(
        before,
        StatusCode::OK,
        "the session must authenticate before the change"
    );

    // Change the password with the same session cookie.
    let (status, _, body) = call(
        app_with_guard(Arc::clone(&guard)),
        put_with_session(&form(OLD_PASSWORD, NEW_PASSWORD, NEW_PASSWORD), &session_id),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::SEE_OTHER,
        "the authenticated change must succeed; body: {body}"
    );

    // The SAME cookie still authenticates afterwards.
    let (after, _, after_body) = call(
        app_with_guard(Arc::clone(&guard)),
        get_with_session("/dashboard", &session_id),
    )
    .await;
    assert_eq!(
        after,
        StatusCode::OK,
        "the same session must stay valid after the password change"
    );
    assert!(
        after_body.contains("Welcome back, ada@example.com"),
        "the same principal must remain projected; body: {after_body}"
    );
}
