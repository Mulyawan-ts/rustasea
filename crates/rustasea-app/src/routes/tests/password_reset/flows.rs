//! Password-reset flow cases (AUTH-013) — positive, negative, and fail-closed.
//!
//! Every case takes the shared [`PROVIDER_LOCK`] because the fixtures install
//! process-wide seams (provider, store, signer, mailer).
//!
//! What is proven:
//!
//! * **Positive** — request `303`s and the [`ArrayMailer`] captures one message
//!   carrying a signed link; following it renders the form (`200`); posting a
//!   valid new password `303`s → `/login`, the new hash verifies, the old does
//!   not.
//! * **Enumeration resistance** — a known and an unknown email yield an
//!   indistinguishable `303`.
//! * **Tamper/expiry** — a flipped signature and an already-expired link are
//!   both `403`.
//! * **Single-use** — replaying a consumed token is `403`.
//! * **Consume-on-failure** — a failing password write still consumes the token,
//!   so the same link cannot be replayed afterwards.
//! * **Validation** — a mismatched confirmation and a policy-violating password
//!   are both `422`.
//! * **Feature gate** — with `reset_passwords` off the routes are `404`.
//! * **Fail closed** — a deny-all store/provider is `500` and no mail is sent.
//!
//! [`ArrayMailer`]: rustasea_mail::ArrayMailer
//! [`PROVIDER_LOCK`]: super::settings_flows::PROVIDER_LOCK

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::http::{header, StatusCode};
use rustasea::auth::users::AuthUserRecord;
use rustasea::auth::verify::{Argon2Verifier, PasswordVerifier};
use rustasea::auth::{
    AuthError, DenyAllProvider, DenyAllResetStore, NewUserRecord, PasswordResetStore, UserProvider,
};

use super::super::settings_flows::PROVIDER_LOCK;
use super::{
    app, array_mailer, call, encode, form_post, get, request_link, reset_body, reset_store,
    seeded_provider, tamper_signature, test_signer, uri_from_link, FORGOT_URI, NEW_PASSWORD,
    OLD_PASSWORD,
};
use crate::routes::auth::password_reset::{install_reset_store, set_reset_passwords_disabled};
use crate::routes::helpers::install_user_provider;

/// Provider decorator: reads delegate to `inner`, but `update_password` fails.
///
/// Models a reset whose signature/token checks succeed and whose hash write
/// fails — the exact case that must still consume the token (F-01). `find_by_email`
/// must keep working so the request reaches the password-write step.
struct FailingWriteProvider {
    inner: Arc<dyn UserProvider>,
}

impl UserProvider for FailingWriteProvider {
    fn find_by_email<'a>(
        &'a self,
        email: &'a str,
    ) -> Pin<
        Box<
            dyn Future<Output = std::result::Result<Option<AuthUserRecord>, AuthError>> + Send + 'a,
        >,
    > {
        self.inner.find_by_email(email)
    }

    fn create<'a>(
        &'a self,
        new_user: NewUserRecord,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<AuthUserRecord, AuthError>> + Send + 'a>>
    {
        self.inner.create(new_user)
    }

    fn update_password<'a>(
        &'a self,
        _user_id: &'a str,
        _password_hash: &'a str,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<(), AuthError>> + Send + 'a>> {
        Box::pin(async move { Err(AuthError::StoreUnavailable) })
    }

    fn set_email_verified_at<'a>(
        &'a self,
        user_id: &'a str,
        verified_at: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<(), AuthError>> + Send + 'a>> {
        self.inner.set_email_verified_at(user_id, verified_at)
    }

    fn update_profile<'a>(
        &'a self,
        user_id: &'a str,
        name: &'a str,
        email: &'a str,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<(), AuthError>> + Send + 'a>> {
        self.inner.update_profile(user_id, name, email)
    }
}

/// Positive: request → signed mail → render form → reset → `303` `/login`.
#[tokio::test]
async fn request_reset_and_consume_rotates_the_hash() {
    let _lock = PROVIDER_LOCK.lock().await;
    let provider = seeded_provider("positive@example.com");
    let _store = reset_store();
    let _signer = test_signer();

    let link = request_link("positive@example.com").await;
    assert!(
        link.contains("/reset-password/"),
        "the mail must carry the signed reset link: {link}"
    );
    let uri = uri_from_link(&link);

    let (rendered, _, _) = call(app(), get(&uri)).await;
    assert_eq!(
        rendered,
        StatusCode::OK,
        "a valid link renders the reset form"
    );

    let before = provider
        .by_email("positive@example.com")
        .expect("user present")
        .password_hash;
    let (status, headers, body) = call(
        app(),
        form_post(
            "/reset-password",
            &reset_body(&uri, NEW_PASSWORD, NEW_PASSWORD),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::SEE_OTHER,
        "reset must succeed; body: {body}"
    );
    assert_eq!(
        headers.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some("/login")
    );

    let after = provider
        .by_email("positive@example.com")
        .expect("user present")
        .password_hash;
    assert_ne!(before, after, "the stored hash must rotate");
    let verifier = Argon2Verifier::new();
    assert!(
        verifier.verify(&after, NEW_PASSWORD),
        "the new password verifies"
    );
    assert!(
        !verifier.verify(&after, OLD_PASSWORD),
        "the old password no longer verifies"
    );
}

/// Negative: a known and an unknown email yield an indistinguishable response.
#[tokio::test]
async fn unknown_email_is_indistinguishable() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _provider = seeded_provider("enum-known@example.com");
    let _store = reset_store();
    let _signer = test_signer();
    let _mailer = array_mailer();

    let known = call(
        app(),
        form_post(
            FORGOT_URI,
            &format!("email={}", encode("enum-known@example.com")),
        ),
    )
    .await;
    let unknown = call(
        app(),
        form_post(
            FORGOT_URI,
            &format!("email={}", encode("enum-unknown@example.com")),
        ),
    )
    .await;

    assert_eq!(known.0, StatusCode::SEE_OTHER);
    assert_eq!(unknown.0, StatusCode::SEE_OTHER);
    assert_eq!(known.1, unknown.1, "the response headers must be identical");
    assert_eq!(known.2, unknown.2, "the response bodies must be identical");
}

/// Negative: a tampered signature is `403` and renders no form.
#[tokio::test]
async fn tampered_link_is_forbidden() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _provider = seeded_provider("tamper@example.com");
    let _store = reset_store();
    let _signer = test_signer();

    let uri = uri_from_link(&request_link("tamper@example.com").await);
    let tampered = tamper_signature(&uri);
    let (status, _, _) = call(app(), get(&tampered)).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a tampered signature is rejected"
    );
}

/// Negative: an already-expired link is `403`.
#[tokio::test]
async fn expired_link_is_forbidden() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _provider = seeded_provider("expired@example.com");
    let _store = reset_store();
    let signer = test_signer();

    let path = "/reset-password/expiredtoken";
    let query = signer.sign(path, signer.now() - 60, &[("email", "expired@example.com")]);
    let uri = format!("{path}?{query}");
    let (status, _, _) = call(app(), get(&uri)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "an expired link is rejected");
}

/// Negative: a consumed token cannot be reused (single-use).
#[tokio::test]
async fn token_cannot_be_reused() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _provider = seeded_provider("reuse@example.com");
    let _store = reset_store();
    let _signer = test_signer();

    let uri = uri_from_link(&request_link("reuse@example.com").await);
    let body = reset_body(&uri, NEW_PASSWORD, NEW_PASSWORD);

    let (first, _, _) = call(app(), form_post("/reset-password", &body)).await;
    assert_eq!(first, StatusCode::SEE_OTHER, "the first reset succeeds");

    let (second, _, _) = call(app(), form_post("/reset-password", &body)).await;
    assert_eq!(
        second,
        StatusCode::FORBIDDEN,
        "a consumed token must be rejected"
    );
}

/// Negative: a mismatched `password_confirmation` is `422`.
#[tokio::test]
async fn mismatched_confirmation_is_rejected() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _provider = seeded_provider("mismatch@example.com");
    let _store = reset_store();
    let _signer = test_signer();

    let uri = uri_from_link(&request_link("mismatch@example.com").await);
    let (status, _, _) = call(
        app(),
        form_post(
            "/reset-password",
            &reset_body(&uri, NEW_PASSWORD, "DifferentPassw0rd!"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

/// Negative: a policy-violating password is `422`.
#[tokio::test]
async fn policy_violating_password_is_rejected() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _provider = seeded_provider("policy@example.com");
    let _store = reset_store();
    let _signer = test_signer();

    let uri = uri_from_link(&request_link("policy@example.com").await);
    let (status, _, _) = call(
        app(),
        form_post(
            "/reset-password",
            &reset_body(&uri, "shortpass", "shortpass"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

/// Feature gate: with `reset_passwords` off the routes are `404`.
#[tokio::test]
async fn disabled_reset_passwords_returns_404() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _provider = seeded_provider("disabled@example.com");
    set_reset_passwords_disabled(true);
    let (page, _, _) = call(app(), get(FORGOT_URI)).await;
    let (post, _, _) = call(
        app(),
        form_post(
            FORGOT_URI,
            &format!("email={}", encode("disabled@example.com")),
        ),
    )
    .await;
    set_reset_passwords_disabled(false);
    assert_eq!(page, StatusCode::NOT_FOUND);
    assert_eq!(post, StatusCode::NOT_FOUND);
}

/// Fail closed: a deny-all store is `500` and no mail is sent.
#[tokio::test]
async fn store_unavailable_fails_closed() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _provider = seeded_provider("store@example.com");
    install_reset_store(Arc::new(DenyAllResetStore));
    let _signer = test_signer();
    let mailer = array_mailer();

    let (status, _, body) = call(
        app(),
        form_post(
            FORGOT_URI,
            &format!("email={}", encode("store@example.com")),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "an unavailable store must fail closed; body: {body}"
    );
    assert_eq!(
        mailer.count(),
        0,
        "a fail-closed request must not send mail"
    );
}

/// Fail closed: a deny-all provider is `500` (never a silent success).
#[tokio::test]
async fn provider_unavailable_fails_closed() {
    let _lock = PROVIDER_LOCK.lock().await;
    install_user_provider(Arc::new(DenyAllProvider));
    let _store = reset_store();
    let _signer = test_signer();
    let mailer = array_mailer();

    let (status, _, body) = call(
        app(),
        form_post(
            FORGOT_URI,
            &format!("email={}", encode("nobody@example.com")),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "an unavailable provider must fail closed; body: {body}"
    );
    assert_eq!(
        mailer.count(),
        0,
        "a fail-closed request must not send mail"
    );
}

/// Negative (F-01): a failing password write still consumes the token.
///
/// The token is deleted BEFORE the hash is written, so a write failure must not
/// leave a replayable link. We install a provider whose `find_by_email` works
/// but whose `update_password` fails, attempt the reset (fail-closed `500`),
/// then restore the healthy provider and replay the SAME body: the token is
/// already gone, so the second attempt is `403`.
#[tokio::test]
async fn failed_password_write_still_consumes_the_token() {
    let _lock = PROVIDER_LOCK.lock().await;
    let provider = seeded_provider("writefail@example.com");
    let _store = reset_store();
    let _signer = test_signer();

    let uri = uri_from_link(&request_link("writefail@example.com").await);
    let body = reset_body(&uri, NEW_PASSWORD, NEW_PASSWORD);

    // The write fails: the reset must fail closed.
    install_user_provider(Arc::new(FailingWriteProvider {
        inner: provider.clone(),
    }));
    let (failed, _, failed_body) = call(app(), form_post("/reset-password", &body)).await;
    assert_eq!(
        failed,
        StatusCode::INTERNAL_SERVER_ERROR,
        "a failing password write must fail closed; body: {failed_body}"
    );

    // The token was consumed before the write, so replaying it is rejected.
    install_user_provider(provider.clone());
    let (replay, _, replay_body) = call(app(), form_post("/reset-password", &body)).await;
    assert_eq!(
        replay,
        StatusCode::FORBIDDEN,
        "the token must be consumed even when the write failed; body: {replay_body}"
    );

    // The hash was never rotated, since the write failed.
    let after = provider
        .by_email("writefail@example.com")
        .expect("user present")
        .password_hash;
    assert!(
        Argon2Verifier::new().verify(&after, OLD_PASSWORD),
        "a failed write must leave the old hash intact"
    );
}

/// The stored token is a HASH, never the plaintext mailed to the user.
#[tokio::test]
async fn stored_token_is_hashed() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _provider = seeded_provider("hashed@example.com");
    let store = reset_store();
    let _signer = test_signer();

    let uri = uri_from_link(&request_link("hashed@example.com").await);
    let token = uri
        .split_once('?')
        .map(|(p, _)| p)
        .unwrap_or(&uri)
        .rsplit('/')
        .next()
        .expect("a token segment")
        .to_string();
    let record = store
        .find("hashed@example.com")
        .await
        .expect("store reachable")
        .expect("a token row exists");
    assert_ne!(record.token_hash, token, "the plaintext must not be stored");
    assert!(
        Argon2Verifier::new().verify(&record.token_hash, &token),
        "the stored hash must verify against the mailed token"
    );
}
