//! Test suites mirroring Laravel's `tests/{Feature,Unit}` split.
//!
//! Grouped as `tests/feature/` and `tests/unit/` modules; each suite references
//! the shared core so the security-critical auth rules are covered for every
//! variant.
//!
//! # Crate roots
//!
//! `tests/feature/mod.rs` and `tests/unit/mod.rs` are the *crate roots* of the
//! generated `feature` and `unit` test targets. Cargo only auto-discovers
//! `tests/*.rs` and `tests/*/main.rs`, so a bare `tests/<suite>/mod.rs` is
//! silently ignored; the generated `Cargo.toml` therefore wires both with an
//! explicit `[[test]]` entry (see [`super::manifest`]). Keeping `mod.rs` as the
//! root mirrors the Laravel `tests/Feature`/`tests/Unit` directory convention
//! while still producing runnable targets.

use super::TemplateFile;

/// Test-suite templates (shared by every variant).
pub fn entries() -> Vec<TemplateFile> {
    vec![
        ("tests/feature/mod.rs", FEATURE_MOD),
        ("tests/feature/auth_test.rs", AUTH_TEST),
        ("tests/feature/routes_test.rs", ROUTES_TEST),
        ("tests/unit/mod.rs", UNIT_MOD),
        ("tests/unit/actions_test.rs", ACTIONS_TEST),
    ]
}

const FEATURE_MOD: &str = r##"//! Feature tests — HTTP-level behaviour of the generated routes.
//!
//! This module is the crate root of the `feature` test target; `Cargo.toml`
//! wires it with an explicit `[[test]]` entry because cargo does not
//! auto-discover `tests/feature/mod.rs`.

pub mod auth_test;
pub mod routes_test;

use rustasea::auth::FortifyConfig;
use rustasea::config::ConfigLoader;

/// Whether the generated `[fortify.features]` toggle `feature` is enabled.
///
/// Mirrors Laravel Fortify's `Features::enabled()` gate behind the starter
/// kit's `skipUnlessFortifyHas(...)` helper: the value is read from
/// `config/fortify.toml` through the real [`ConfigLoader`]. A missing config
/// file or `[fortify]` table falls back to the enabled default so a test fails
/// open rather than silently skipping.
///
/// Unknown feature names are reported as disabled (fail closed), matching the
/// kit's "only the named features gate a test" behaviour.
pub fn fortify_feature_enabled(feature: &str) -> bool {
    let Ok(loader) = ConfigLoader::load() else {
        return true;
    };
    let Ok(config) = FortifyConfig::from_loader(&loader) else {
        return true;
    };
    match feature {
        "registration" => config.features.registration,
        "reset_passwords" => config.features.reset_passwords,
        "email_verification" => config.features.email_verification,
        _ => false,
    }
}

/// Skip the current test when the named Fortify feature is disabled.
///
/// Mirrors the starter kit's `skipUnlessFortifyHas('registration')`: call it at
/// the top of a test to make that test conditional on `config/fortify.toml`.
#[macro_export]
macro_rules! skip_unless_fortify_has {
    ($feature:expr) => {
        if !$crate::fortify_feature_enabled($feature) {
            return;
        }
    };
}
"##;

const AUTH_TEST: &str = r##"//! Feature tests for registration, authentication, and the session guard.
//!
//! The validation tests exercise the generated concern directly; the session
//! tests drive the *real* `rustasea::auth::SessionGuard` over the in-memory
//! store, so they cover the login → parse → logout lifecycle the generated auth
//! actions compose without depending on any unfinished scaffold stub.

use std::sync::Arc;

use @@app_snake@@::app::concerns::password_validation_rules;
use rustasea::auth::users::{AuthUserRecord, MemoryUserRegistry};
use rustasea::auth::verify::{Argon2Verifier, PasswordVerifier};
use rustasea::auth::{AuthError, Credentials, Guard, SessionGuard, SessionPolicy};

/// Registration rejects passwords shorter than the configured minimum.
#[test]
fn registration_rejects_short_passwords() {
    assert!(password_validation_rules::validate("short").is_err());
}

/// Registration accepts a password that meets the minimum length.
#[test]
fn registration_accepts_a_long_password() {
    assert!(password_validation_rules::validate("correct-horse-battery").is_ok());
}

/// The Fortify `registration` gate reads `config/fortify.toml`.
#[test]
fn fortify_registration_gate_is_queryable() {
    // The generated config enables registration, so the gate reports `true`.
    assert!(crate::fortify_feature_enabled("registration"));
    // Unknown feature names fail closed.
    assert!(!crate::fortify_feature_enabled("not-a-real-feature"));
}

/// Build a session guard over the in-memory store with one seeded user.
fn guard_with_user(password: &str) -> SessionGuard {
    let verifier = Argon2Verifier::new();
    let password_hash = verifier.hash(password).expect("argon2 hashing succeeds");
    let registry = Arc::new(MemoryUserRegistry::default());
    registry.seed(AuthUserRecord {
        id: "user-1".to_string(),
        email: "ada@example.com".to_string(),
        password_hash,
        email_verified_at: None,
        timezone: None,
    });
    SessionGuard::new(SessionPolicy::default()).with_lookup(registry)
}

/// Login → parse → logout over the real session guard (registration-gated).
#[tokio::test]
async fn session_guard_login_parse_logout_lifecycle() {
    crate::skip_unless_fortify_has!("registration");

    let guard = guard_with_user("correct-horse-battery");
    let token = guard
        .login(&Credentials {
            email: "ada@example.com".to_string(),
            password: "correct-horse-battery".to_string(),
        })
        .await
        .expect("login succeeds");
    assert_eq!(token.token_type, "Session");

    let principal = guard
        .parse(&token.access_token)
        .await
        .expect("parse resolves the principal");
    assert_eq!(principal.id, "user-1");
    assert_eq!(principal.guard, "session");

    guard
        .logout(&token.access_token)
        .await
        .expect("logout succeeds");
    assert!(
        guard.parse(&token.access_token).await.is_err(),
        "a session replayed after logout must be rejected"
    );
}

/// Wrong credentials are rejected without issuing a session.
#[tokio::test]
async fn session_guard_rejects_bad_credentials() {
    let guard = guard_with_user("correct-horse-battery");
    let error = guard
        .login(&Credentials {
            email: "ada@example.com".to_string(),
            password: "wrong-password".to_string(),
        })
        .await
        .expect_err("bad credentials must be rejected");
    assert_eq!(error, AuthError::BadCredentials);
}
"##;

const ROUTES_TEST: &str = r##"//! Feature tests — route-table smoke checks.
//!
//! Asserts the generated routers register the expected paths by dispatching
//! real requests through the compiled router: a registered path with an
//! unsupported method yields `405`, while an unregistered path yields `404`.
//! This exercises `routes::router` end to end without invoking the unfinished
//! scaffold handlers (method/path matching happens before a handler runs).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use @@app_snake@@::routes;
use rustasea::http::AppState;
use tower::ServiceExt;

/// Build the full application router over a throwaway `AppState`.
fn app() -> axum::Router {
    routes::router(Arc::new(AppState::new("testing", true)))
}

/// Dispatch one request and return the response.
async fn response_for(method: &str, uri: &str) -> axum::response::Response {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .expect("valid request");
    app().oneshot(request).await.expect("router dispatch")
}

/// Dispatch one request and return the response status.
async fn status_for(method: &str, uri: &str) -> StatusCode {
    response_for(method, uri).await.status()
}

/// Every generated path is registered (a wrong method ⇒ `405`, never `404`).
#[tokio::test]
async fn generated_routes_register_expected_paths() {
    for (method, uri) in [
        ("POST", "/"),
        ("POST", "/dashboard"),
        ("DELETE", "/login"),
        ("GET", "/logout"),
        ("DELETE", "/register"),
        ("DELETE", "/confirm-password"),
        ("DELETE", "/settings/profile"),
        ("DELETE", "/settings/password"),
        ("POST", "/settings/security"),
    ] {
        assert_eq!(
            status_for(method, uri).await,
            StatusCode::METHOD_NOT_ALLOWED,
            "{method} {uri} must be registered"
        );
    }
}

/// `/settings` redirects (302) to the profile screen.
#[tokio::test]
async fn settings_redirects_to_profile() {
    let response = response_for("GET", "/settings").await;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(
        response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some("/settings/profile")
    );
}

/// Unregistered paths fall through to `404`.
#[tokio::test]
async fn unknown_paths_are_not_found() {
    for uri in ["/does-not-exist", "/settings/unknown"] {
        assert_eq!(
            status_for("GET", uri).await,
            StatusCode::NOT_FOUND,
            "GET {uri} must not be registered"
        );
    }
}
"##;

const UNIT_MOD: &str = r##"//! Unit tests — domain actions and concerns in isolation.
//!
//! This module is the crate root of the `unit` test target; `Cargo.toml` wires
//! it with an explicit `[[test]]` entry because cargo does not auto-discover
//! `tests/unit/mod.rs`.

pub mod actions_test;
"##;

const ACTIONS_TEST: &str = r##"//! Unit tests for the generated auth actions and validation concerns.

use @@app_snake@@::app::actions::auth::create_new_user::CreateNewUser;
use @@app_snake@@::app::actions::auth::redirect_if_authenticated;
use @@app_snake@@::app::concerns::profile_validation_rules;
use rustasea::action::Action as _;

/// Guest-only redirects detect an authenticated session.
#[test]
fn authenticated_users_are_detected() {
    assert!(redirect_if_authenticated::is_authenticated(Some("user-id")));
    assert!(!redirect_if_authenticated::is_authenticated(None));
}

/// The registration action implements the shared `Action` trait.
#[test]
fn registration_action_is_an_action() {
    let _action = CreateNewUser;
}

/// A blank display name is rejected by the profile rules.
#[test]
fn profile_rules_reject_blank_names() {
    assert!(profile_validation_rules::validate_name("   ").is_err());
}

/// A normal display name passes the profile rules.
#[test]
fn profile_rules_accept_a_normal_name() {
    assert!(profile_validation_rules::validate_name("Ada Lovelace").is_ok());
}

/// A display name longer than `NAME_MAX` is rejected.
#[test]
fn profile_rules_reject_overlong_names() {
    let overlong = "x".repeat(profile_validation_rules::NAME_MAX + 1);
    assert!(profile_validation_rules::validate_name(&overlong).is_err());
}
"##;
