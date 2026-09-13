//! Registration-flow tests (AUTH-009).
//!
//! These pin the behaviour of `POST /register` (`register_submit` in
//! [`crate::routes::auth`]) against a real [`MemoryUserProvider`] and a real
//! [`SessionGuard`] — no mock returning hardcoded values.
//!
//! What is proven:
//!
//! * **Positive** — a valid payload creates the user (a verifiable Argon2 hash
//!   is retrievable through the provider), auto-logs them in (a session cookie
//!   is set), and `303` → `[fortify].home`.
//! * **Validation** — duplicate email, a policy-violating password, a
//!   mismatched confirmation, and a missing name each answer `422` with a field
//!   error.
//! * **CSRF** — a `POST /register` with no token is rejected `403`, proving the
//!   CSRF gate covers the route.
//! * **Fail-closed** — with [`DenyAllProvider`] installed the uniqueness probe
//!   cannot resolve, so validation fails with a typed missing-context error
//!   instead of a silent success.
//! * **Feature gate** — with `[fortify].features.registration` disabled the
//!   route answers `404` (the kit removes the routes entirely).
//!
//! The [`UserProvider`] seam and the registration feature override are
//! process-wide, so provider/feature tests are serialized through the shared
//! [`PROVIDER_LOCK`] to stay deterministic under libtest's parallel runner.
//!
//! [`MemoryUserProvider`]: rustasea::auth::MemoryUserProvider
//! [`DenyAllProvider`]: rustasea::auth::DenyAllProvider
//! [`UserProvider`]: rustasea::auth::UserProvider

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderValue, Request, StatusCode};
use axum::Router;
use rustasea::auth::users::AuthUserRecord;
use rustasea::auth::verify::{Argon2Verifier, PasswordVerifier};
use rustasea::auth::{DenyAllProvider, MemoryUserProvider, SessionGuard, SessionPolicy};
use rustasea::http::AppState;
use rustasea::validation::serde_json::{self, Value};

use super::{app, call, csrf_same_origin, method_request};
use crate::routes::auth::set_registration_disabled;
use crate::routes::helpers::install_user_provider;
use crate::routes::{compile, table};

/// The registration endpoint under test.
const REGISTER_URI: &str = "/register";

/// A password that satisfies [`PasswordPolicy::production`] (>=12 chars, mixed
/// case, letters, numbers, a symbol).
const VALID_PASSWORD: &str = "Sup3rSecret!Pass";

/// The email a positive registration uses.
const NEW_EMAIL: &str = "newcomer@example.com";

/// Serializes the provider/feature-installing tests in this module.
///
/// Shared with the sibling [`super::settings_flows`] / [`super::settings_password`]
/// modules so every provider-installing test is mutually exclusive.
fn provider_lock() -> &'static tokio::sync::Mutex<()> {
    &super::settings_flows::PROVIDER_LOCK
}

/// Build a registration `POST` request carrying a urlencoded body.
fn register_post(body: &str) -> Request<Body> {
    let mut request = method_request("POST", REGISTER_URI);
    request.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    *request.body_mut() = Body::from(body.to_string());
    request
}

/// Build the served router with a fresh [`SessionGuard`] installed.
fn app_with_guard() -> Router {
    let guard = SessionGuard::new(SessionPolicy::default());
    compile(
        table(),
        Arc::new(AppState::new("testing", true).with_auth(Arc::new(guard))),
    )
}

/// Seed one user with a real Argon2 hash for `email` into a fresh provider.
fn provider_with(email: &str) -> Arc<MemoryUserProvider> {
    let provider = MemoryUserProvider::default();
    provider.seed(AuthUserRecord {
        id: "existing-user".to_string(),
        email: email.to_string(),
        password_hash: Argon2Verifier::new()
            .hash("existing-secret")
            .expect("hash the seeded password"),
        email_verified_at: Some("2026-01-01T00:00:00Z".to_string()),
    });
    Arc::new(provider)
}

/// RAII guard that clears the registration feature override on drop, so a
/// panicking assertion cannot leak the process-wide flag into other tests.
struct FeatureGate;

impl FeatureGate {
    /// Disable registration for the lifetime of the guard.
    fn disabled() -> Self {
        set_registration_disabled(true);
        Self
    }
}

impl Drop for FeatureGate {
    fn drop(&mut self) {
        set_registration_disabled(false);
    }
}

/// The `email` field errors parsed from a `422` validation body.
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

/// Positive: a valid registration creates the user, logs them in, and `303` →
/// `/dashboard`; the stored hash verifies against the submitted password.
#[tokio::test]
async fn valid_registration_creates_logs_in_and_redirects() {
    let _guard = provider_lock().lock().await;
    // Ensure the feature gate is on regardless of test ordering.
    set_registration_disabled(false);
    let provider = Arc::new(MemoryUserProvider::default());
    install_user_provider(provider.clone());

    let body = format!("name=Ada&email={NEW_EMAIL}&password={VALID_PASSWORD}&password_confirmation={VALID_PASSWORD}");
    let (status, headers, response_body) =
        call(app_with_guard(), csrf_same_origin(register_post(&body))).await;

    assert_eq!(
        status,
        StatusCode::SEE_OTHER,
        "a valid registration must redirect; body: {response_body}"
    );
    assert_eq!(
        headers.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some("/dashboard"),
        "success must redirect to the configured home"
    );
    assert!(
        headers.get(header::SET_COOKIE).is_some(),
        "registration must auto-login (set the session cookie)"
    );

    let record = provider
        .by_email(NEW_EMAIL)
        .expect("the new user must be persisted");
    assert!(
        Argon2Verifier::new().verify(&record.password_hash, VALID_PASSWORD),
        "the stored hash must verify against the submitted password"
    );
    assert!(
        record.email_verified_at.is_none(),
        "a fresh registration lands unverified so the `verified` gate sends it to /verify-email"
    );
}

/// Negative: an email already owned by another user is rejected `422` with an
/// `email` field error (registration passes no `ignore_id`).
#[tokio::test]
async fn duplicate_email_is_rejected() {
    let _guard = provider_lock().lock().await;
    set_registration_disabled(false);
    install_user_provider(provider_with(NEW_EMAIL));

    let body = format!("name=Ada&email={NEW_EMAIL}&password={VALID_PASSWORD}&password_confirmation={VALID_PASSWORD}");
    let (status, _, response_body) =
        call(app_with_guard(), csrf_same_origin(register_post(&body))).await;

    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a duplicate email must be rejected; body: {response_body}"
    );
    assert!(
        !field_errors(&response_body, "email").is_empty(),
        "a field error on `email` is required; body: {response_body}"
    );
}

/// Negative: a password below the production policy minimum is rejected `422`.
#[tokio::test]
async fn short_password_is_rejected() {
    let _guard = provider_lock().lock().await;
    set_registration_disabled(false);
    install_user_provider(Arc::new(MemoryUserProvider::default()));

    let body = "name=Ada&email=short@example.com&password=Ab1!&password_confirmation=Ab1!";
    let (status, _, response_body) =
        call(app_with_guard(), csrf_same_origin(register_post(body))).await;

    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a policy-violating password must be rejected; body: {response_body}"
    );
    assert!(
        !field_errors(&response_body, "password").is_empty(),
        "a field error on `password` is required; body: {response_body}"
    );
}

/// Negative: a mismatched confirmation is rejected `422`.
#[tokio::test]
async fn mismatched_confirmation_is_rejected() {
    let _guard = provider_lock().lock().await;
    set_registration_disabled(false);
    install_user_provider(Arc::new(MemoryUserProvider::default()));

    let body = format!(
        "name=Ada&email=mismatch@example.com&password={VALID_PASSWORD}&password_confirmation=Something!Else1"
    );
    let (status, _, response_body) =
        call(app_with_guard(), csrf_same_origin(register_post(&body))).await;

    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a mismatched confirmation must be rejected; body: {response_body}"
    );
    assert!(
        !field_errors(&response_body, "password").is_empty(),
        "a field error on `password` is required; body: {response_body}"
    );
}

/// Negative: a missing name is rejected `422`.
#[tokio::test]
async fn missing_name_is_rejected() {
    let _guard = provider_lock().lock().await;
    set_registration_disabled(false);
    install_user_provider(Arc::new(MemoryUserProvider::default()));

    let body = format!(
        "email=noname@example.com&password={VALID_PASSWORD}&password_confirmation={VALID_PASSWORD}"
    );
    let (status, _, response_body) =
        call(app_with_guard(), csrf_same_origin(register_post(&body))).await;

    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a missing name must be rejected; body: {response_body}"
    );
    assert!(
        !field_errors(&response_body, "name").is_empty(),
        "a field error on `name` is required; body: {response_body}"
    );
}

/// Negative: a `POST /register` with no CSRF token is rejected `403` (the gate
/// covers the route) even when it claims `same-origin`.
#[tokio::test]
async fn missing_csrf_token_is_rejected() {
    let _guard = provider_lock().lock().await;
    set_registration_disabled(false);
    install_user_provider(Arc::new(MemoryUserProvider::default()));

    let mut request = register_post("name=Ada&email=csrf@example.com&password=Sup3rSecret!Pass&password_confirmation=Sup3rSecret!Pass");
    request
        .headers_mut()
        .insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
    let (status, headers, _) = call(app_with_guard(), request).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a token-less registration must be rejected by the CSRF gate"
    );
    assert!(
        headers.get(header::SET_COOKIE).is_none(),
        "a rejected registration must not set a cookie"
    );
}

/// Fail-closed: with [`DenyAllProvider`] installed the uniqueness probe cannot
/// resolve, so validation fails with a typed missing-context error (`422`)
/// rather than silently creating the user.
#[tokio::test]
async fn provider_unavailable_fails_closed() {
    let _guard = provider_lock().lock().await;
    set_registration_disabled(false);
    install_user_provider(Arc::new(DenyAllProvider));

    let body = format!("name=Ada&email=denied@example.com&password={VALID_PASSWORD}&password_confirmation={VALID_PASSWORD}");
    let (status, headers, response_body) =
        call(app_with_guard(), csrf_same_origin(register_post(&body))).await;

    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "an unavailable provider must fail closed, never a silent success; body: {response_body}"
    );
    assert!(
        headers.get(header::SET_COOKIE).is_none(),
        "a fail-closed registration must not set a cookie"
    );
    let errors = field_errors(&response_body, "email");
    assert!(
        errors
            .iter()
            .any(|message| message.contains("could not be validated")),
        "must be the typed missing-context failure, not a silent pass; errors: {errors:?}"
    );
}

/// Feature gate: with `[fortify].features.registration` disabled the route
/// answers `404` — the closest honest analogue to the kit removing the routes.
#[tokio::test]
async fn disabled_registration_returns_404() {
    let _guard = provider_lock().lock().await;
    let _gate = FeatureGate::disabled();
    install_user_provider(Arc::new(MemoryUserProvider::default()));

    let body = format!("name=Ada&email=disabled@example.com&password={VALID_PASSWORD}&password_confirmation={VALID_PASSWORD}");
    let (status, headers, _) = call(app_with_guard(), csrf_same_origin(register_post(&body))).await;

    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a disabled registration feature must answer 404"
    );
    assert!(
        headers.get(header::SET_COOKIE).is_none(),
        "a disabled registration must not set a cookie"
    );
}

/// The static `/register` GET route still renders (registration is enabled by
/// default); a regression pin for the page handler.
#[tokio::test]
async fn register_page_renders() {
    let _guard = provider_lock().lock().await;
    set_registration_disabled(false);

    let (status, _, body) = call(app(), super::get(REGISTER_URI)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("Register"),
        "the registration form must render; body: {body}"
    );
}
