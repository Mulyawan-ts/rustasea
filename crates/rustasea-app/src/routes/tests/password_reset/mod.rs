//! Password-reset tests (AUTH-013).
//!
//! Drives the compiled router against a real [`MemoryUserProvider`] and a real
//! [`MemoryPasswordResetStore`], both seeded/injected through the process-wide
//! seams (so every test serializes through the shared `PROVIDER_LOCK`). The
//! signed-link and mailer overrides are process-wide too.
//!
//! The cases live in the sibling [`flows`] module; this parent holds the shared
//! fixtures and request/parse helpers so each file stays under the 500-line cap.
//!
//! [`ArrayMailer`]: rustasea_mail::ArrayMailer
//! [`MemoryUserProvider`]: rustasea::auth::MemoryUserProvider
//! [`MemoryPasswordResetStore`]: rustasea::auth::MemoryPasswordResetStore

mod flows;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderValue, Request};
use rustasea::auth::users::AuthUserRecord;
use rustasea::auth::verify::{Argon2Verifier, PasswordVerifier};
use rustasea::auth::{MemoryPasswordResetStore, MemoryUserProvider, SignedUrlSigner};
use rustasea_mail::{ArrayMailer, Mail};

use super::{app, call, csrf_same_origin, method_request};
use crate::routes::auth::password_reset::{install_reset_store, install_signer};
use crate::routes::helpers::install_user_provider;

/// The request endpoint under test.
pub(super) const FORGOT_URI: &str = "/forgot-password";

/// The seeded old password (`seeded_provider` hashes it for the user).
pub(super) const OLD_PASSWORD: &str = "secret-for-user";

/// A password satisfying [`PasswordPolicy::production`].
///
/// [`PasswordPolicy::production`]: rustasea::validation::PasswordPolicy::production
pub(super) const NEW_PASSWORD: &str = "BrandNewPassw0rd!";

/// Seed a provider with one verified user, install it, and return it.
pub(super) fn seeded_provider(email: &str) -> Arc<MemoryUserProvider> {
    let provider = MemoryUserProvider::default();
    provider.seed(AuthUserRecord {
        id: format!("id-{email}"),
        email: email.to_string(),
        password_hash: Argon2Verifier::new()
            .hash(OLD_PASSWORD)
            .expect("hash the seeded password"),
        email_verified_at: Some("2026-01-01T00:00:00Z".to_string()),
        timezone: None,
    });
    let provider = Arc::new(provider);
    install_user_provider(provider.clone());
    provider
}

/// Install a fresh [`ArrayMailer`] and return the recording handle.
pub(super) fn array_mailer() -> ArrayMailer {
    let mailer = ArrayMailer::new();
    Mail::set_mailer(Arc::new(mailer.clone()));
    mailer
}

/// Install a deterministic signer and return it.
pub(super) fn test_signer() -> Arc<SignedUrlSigner> {
    let signer = Arc::new(SignedUrlSigner::new("test-reset-key"));
    install_signer(Some(Arc::clone(&signer)));
    signer
}

/// Install a fresh in-memory reset store and return it.
pub(super) fn reset_store() -> Arc<MemoryPasswordResetStore> {
    let store = Arc::new(MemoryPasswordResetStore::default());
    install_reset_store(store.clone());
    store
}

/// Build a urlencoded POST request carrying the CSRF token + same-origin signal.
///
/// Both reset POSTs are on the CSRF allow-list, so the tests must pass the gate
/// exactly as a real browser form would.
pub(super) fn form_post(uri: &str, body: &str) -> Request<Body> {
    let mut request = method_request("POST", uri);
    request.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    *request.body_mut() = Body::from(body.to_string());
    csrf_same_origin(request)
}

/// A plain GET request (re-exported for the link-follow steps).
pub(super) fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("build")
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
pub(super) fn uri_from_link(link: &str) -> String {
    let index = link
        .find("/reset-password")
        .expect("the link carries the path");
    link[index..].to_string()
}

/// Read one query parameter from a signed-link URI (percent-decoded).
fn param(uri: &str, key: &str) -> String {
    let query = uri.split_once('?').map(|(_, q)| q).unwrap_or("");
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return percent_decode(v);
            }
        }
    }
    panic!("missing query parameter {key} in {uri}");
}

/// The `{token}` path segment of a reset URI.
fn token_from_uri(uri: &str) -> String {
    let path = uri.split_once('?').map(|(p, _)| p).unwrap_or(uri);
    path.rsplit('/')
        .next()
        .expect("a token segment")
        .to_string()
}

/// Minimal percent-decoding (`%XX` and `+`-for-space).
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
                match (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                    (Some(hi), Some(lo)) => {
                        out.push(hi * 16 + lo);
                        index += 3;
                    }
                    _ => {
                        out.push(b'%');
                        index += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Flip the first character of the `signature=` value in `uri`.
pub(super) fn tamper_signature(uri: &str) -> String {
    let marker = "signature=";
    let index = uri.find(marker).expect("a signature is present") + marker.len();
    let mut bytes = uri.as_bytes().to_vec();
    bytes[index] = if bytes[index] == b'a' { b'b' } else { b'a' };
    String::from_utf8(bytes).expect("the URI stays ASCII")
}

/// Run the request flow and return the signed reset URI captured from the mail.
pub(super) async fn request_link(email: &str) -> String {
    let mailer = array_mailer();
    let (status, _, body) = call(
        app(),
        form_post(FORGOT_URI, &format!("email={}", encode(email))),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::SEE_OTHER,
        "the request must redirect; body: {body}"
    );
    let message = mailer.last().expect("a reset mail was recorded");
    link_from_body(message.html.as_deref().unwrap_or_default())
}

/// Percent-encode an email for a form body (only `@` matters here).
pub(super) fn encode(email: &str) -> String {
    email.replace('@', "%40")
}

/// Build the reset POST body from a signed link + a password pair.
pub(super) fn reset_body(uri: &str, password: &str, confirmation: &str) -> String {
    format!(
        "email={}&token={}&expires={}&signature={}&password={}&password_confirmation={}",
        param(uri, "email"),
        token_from_uri(uri),
        param(uri, "expires"),
        param(uri, "signature"),
        encode(password),
        encode(confirmation),
    )
}
