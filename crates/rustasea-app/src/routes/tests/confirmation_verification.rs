//! Password-confirmation (AUTH-011) and email-verification (AUTH-014) tests.
//!
//! The suites are split into [`confirmation`] and [`verification`] siblings so
//! each file stays under the 500-line cap; this parent holds the shared
//! fixtures. Both suites drive the compiled router against a real
//! [`SessionGuard`] and a real [`MemoryUserProvider`] seeded with genuine Argon2
//! hashes — no mock returning hardcoded values. The provider cell and the
//! mailer/signer overrides are process-wide, so every test serializes through
//! the shared [`PROVIDER_LOCK`].
//!
//! [`SessionGuard`]: rustasea::auth::SessionGuard
//! [`MemoryUserProvider`]: rustasea::auth::MemoryUserProvider
//! [`PROVIDER_LOCK`]: super::settings_flows::PROVIDER_LOCK

mod confirmation;
mod verification;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderValue, Request};
use axum::Router;
use rustasea::auth::users::{AuthUserRecord, MemoryUserRegistry};
use rustasea::auth::verify::{Argon2Verifier, PasswordVerifier};
use rustasea::auth::{Guard, MemoryUserProvider, SessionGuard, SessionPolicy, SignedUrlSigner};
use rustasea::http::AppState;
use rustasea::validation::serde_json::{self, Value};
use rustasea_mail::{ArrayMailer, Mail};

use super::{csrf_same_origin, method_request};
use crate::routes::auth::verification::{install_signer, set_email_verification_disabled};
use crate::routes::helpers::install_user_provider;
use crate::routes::{compile, table, SESSION_COOKIE_NAME};

/// The confirm-password endpoint under test.
const CONFIRM_URI: &str = "/confirm-password";

/// The resend endpoint under test.
const RESEND_URI: &str = "/email/verification-notification";

/// The `password.confirm`-gated page used to observe gate state.
const SECURITY_URI: &str = "/settings/security";

/// The seeded password for `user-a` (`record` hashes `secret-for-<id>`).
const USER_A_PASSWORD: &str = "secret-for-user-a";

/// The seeded email for `user-a`.
const USER_A_EMAIL: &str = "ada@example.com";

/// Seed a provider with an **unverified** `user-a` and install it into the seam.
///
/// Distinct from [`super::settings_flows::seeded_provider`] (which seeds a
/// verified user): verification tests need `email_verified_at` to start `None`
/// so a successful verify is observable.
fn unverified_provider() -> Arc<MemoryUserProvider> {
    let provider = MemoryUserProvider::default();
    provider.seed(AuthUserRecord {
        id: "user-a".to_string(),
        email: USER_A_EMAIL.to_string(),
        password_hash: Argon2Verifier::new()
            .hash(USER_A_PASSWORD)
            .expect("hash the seeded password"),
        email_verified_at: None,
    });
    let provider = Arc::new(provider);
    install_user_provider(provider.clone());
    provider
}

/// Seed a provider with a verified `user-a` (for the confirmation tests, where
/// the password is what matters).
fn verified_provider() -> Arc<MemoryUserProvider> {
    let provider = MemoryUserProvider::default();
    provider.seed(AuthUserRecord {
        id: "user-a".to_string(),
        email: USER_A_EMAIL.to_string(),
        password_hash: Argon2Verifier::new()
            .hash(USER_A_PASSWORD)
            .expect("hash the seeded password"),
        email_verified_at: Some("2026-01-01T00:00:00Z".to_string()),
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

/// Mint a session id for `user-a`.
async fn session_for_user_a(guard: &SessionGuard) -> String {
    guard
        .login_using_id("user-a")
        .await
        .expect("mint a session for user-a")
        .access_token
}

/// A urlencoded POST carrying the session cookie + the CSRF signal.
fn post_with_session(uri: &str, body: &str, session_id: &str) -> Request<Body> {
    let mut request = method_request("POST", uri);
    request.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    request.headers_mut().insert(
        header::COOKIE,
        HeaderValue::from_str(&format!("{SESSION_COOKIE_NAME}={session_id}"))
            .expect("valid cookie header"),
    );
    *request.body_mut() = Body::from(body.to_string());
    csrf_same_origin(request)
}

/// A plain GET carrying the session cookie for `session_id`.
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

/// Install a fresh [`ArrayMailer`] and return the recording handle.
fn array_mailer() -> ArrayMailer {
    let mailer = ArrayMailer::new();
    Mail::set_mailer(Arc::new(mailer.clone()));
    mailer
}

/// Install a deterministic signed-URL signer and return it.
fn test_signer() -> Arc<SignedUrlSigner> {
    let signer = Arc::new(SignedUrlSigner::new("test-verification-key"));
    install_signer(Some(Arc::clone(&signer)));
    signer
}

/// Extract the first `href="..."` target from an HTML mail body.
fn link_from_body(body: &str) -> String {
    let marker = "href=\"";
    let start = body.find(marker).expect("the mail body carries a link") + marker.len();
    let rest = &body[start..];
    let end = rest.find('"').expect("the href terminates");
    rest[..end].to_string()
}

/// Reduce an absolute link to the path + query the router dispatches on.
fn uri_from_link(link: &str) -> String {
    let index = link
        .find("/email/verify")
        .expect("the link carries the verify path");
    link[index..].to_string()
}

/// Flip the first character of the `signature=` value in `uri`.
fn tamper_signature(uri: &str) -> String {
    let marker = "signature=";
    let index = uri.find(marker).expect("a signature is present") + marker.len();
    let mut bytes = uri.as_bytes().to_vec();
    bytes[index] = if bytes[index] == b'a' { b'b' } else { b'a' };
    String::from_utf8(bytes).expect("the URI stays ASCII")
}

/// The `password` field errors parsed from a `422` body.
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

/// RAII guard that clears the email-verification override on drop.
struct VerificationGate;

impl VerificationGate {
    /// Disable email verification for the guard's lifetime.
    fn disabled() -> Self {
        set_email_verification_disabled(true);
        Self
    }
}

impl Drop for VerificationGate {
    fn drop(&mut self) {
        set_email_verification_disabled(false);
    }
}
