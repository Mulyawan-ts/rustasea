//! Email-verification tests (AUTH-014).
//!
//! * **Send** — resend `303`s and the [`ArrayMailer`] captures exactly one
//!   message whose body carries a signed link.
//! * **Verify** — following that link `303`s → `/dashboard` and flips the
//!   stored `email_verified_at` to `Some`.
//! * **Tamper/expiry** — a flipped signature character and an already-expired
//!   link are both `403`, leaving the address unverified.
//! * **Auth gate** — an unauthenticated resend is `302` → `/login`.
//! * **Feature gate** — with `email_verification` disabled the route is `404`.
//! * **Fail closed** — with [`DenyAllProvider`] installed the resend is `500`
//!   and no mail is sent.
//!
//! [`ArrayMailer`]: rustasea_mail::ArrayMailer
//! [`DenyAllProvider`]: rustasea::auth::DenyAllProvider

use std::sync::Arc;

use axum::http::{header, StatusCode};
use rustasea::auth::DenyAllProvider;

use super::super::settings_flows::PROVIDER_LOCK;
use super::super::{app, call, csrf_same_origin, method_request};
use super::{
    app_with_guard, array_mailer, guard_with_user_a, link_from_body, post_with_session,
    session_for_user_a, tamper_signature, test_signer, unverified_provider, uri_from_link,
    VerificationGate, RESEND_URI, USER_A_EMAIL,
};
use crate::routes::auth::verification::email_hash;
use crate::routes::helpers::install_user_provider;

/// Positive: resend `303`s, mails exactly one signed link, and following that
/// link `303`s → `/dashboard` and marks the address verified.
#[tokio::test]
async fn resend_mails_a_signed_link_that_verifies() {
    let _lock = PROVIDER_LOCK.lock().await;
    let provider = unverified_provider();
    let mailer = array_mailer();
    let _signer = test_signer();
    let guard = guard_with_user_a();
    let session = session_for_user_a(&guard).await;

    let (status, _, body) = call(
        app_with_guard(Arc::clone(&guard)),
        post_with_session(RESEND_URI, "", &session),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::SEE_OTHER,
        "resend must redirect to the notice; body: {body}"
    );
    assert_eq!(mailer.count(), 1, "exactly one verification mail is sent");

    let message = mailer.last().expect("a message was recorded");
    let link = link_from_body(message.html.as_deref().unwrap_or_default());
    assert!(
        link.contains("/email/verify/"),
        "the mail body must carry the signed verification link: {link}"
    );

    let (verified, headers, _) = call(app(), super::super::get(&uri_from_link(&link))).await;
    assert_eq!(
        verified,
        StatusCode::SEE_OTHER,
        "a valid signed link must be accepted"
    );
    assert_eq!(
        headers.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some("/dashboard")
    );
    assert!(
        provider
            .by_id("user-a")
            .expect("user-a is present")
            .email_verified_at
            .is_some(),
        "the address must be marked verified"
    );
}

/// Negative: flipping a character in the signature is `403` and the address
/// stays unverified.
#[tokio::test]
async fn tampered_link_is_forbidden_and_does_not_verify() {
    let _lock = PROVIDER_LOCK.lock().await;
    let provider = unverified_provider();
    let mailer = array_mailer();
    let _signer = test_signer();
    let guard = guard_with_user_a();
    let session = session_for_user_a(&guard).await;

    let _ = call(
        app_with_guard(Arc::clone(&guard)),
        post_with_session(RESEND_URI, "", &session),
    )
    .await;
    let message = mailer.last().expect("a message was recorded");
    let link = link_from_body(message.html.as_deref().unwrap_or_default());
    let tampered = tamper_signature(&uri_from_link(&link));

    let (status, _, _) = call(app(), super::super::get(&tampered)).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a tampered signature must be rejected"
    );
    assert!(
        provider
            .by_id("user-a")
            .expect("user-a is present")
            .email_verified_at
            .is_none(),
        "a tampered link must not verify the address"
    );
}

/// Negative: a link signed with an expiry in the past is `403`.
#[tokio::test]
async fn expired_link_is_forbidden() {
    let _lock = PROVIDER_LOCK.lock().await;
    let provider = unverified_provider();
    let signer = test_signer();

    let path = format!("/email/verify/user-a/{}", email_hash(USER_A_EMAIL));
    let query = signer.sign(&path, signer.now() - 60, &[("email", USER_A_EMAIL)]);
    let uri = format!("{path}?{query}");

    let (status, _, _) = call(app(), super::super::get(&uri)).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "an expired link must be rejected"
    );
    assert!(
        provider
            .by_id("user-a")
            .expect("user-a is present")
            .email_verified_at
            .is_none(),
        "an expired link must not verify the address"
    );
}

/// Negative: an unauthenticated resend is `302` → `/login`.
#[tokio::test]
async fn unauthenticated_resend_redirects_to_login() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _provider = unverified_provider();
    let _mailer = array_mailer();
    let _signer = test_signer();

    let (status, headers, _) =
        call(app(), csrf_same_origin(method_request("POST", RESEND_URI))).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        headers.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some("/login")
    );
}

/// Feature gate: with `email_verification` disabled the resend route is `404`.
#[tokio::test]
async fn disabled_email_verification_returns_404() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _provider = unverified_provider();
    let _gate = VerificationGate::disabled();
    let guard = guard_with_user_a();
    let session = session_for_user_a(&guard).await;

    let (status, _, _) = call(
        app_with_guard(Arc::clone(&guard)),
        post_with_session(RESEND_URI, "", &session),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a disabled email-verification feature must answer 404"
    );
}

/// Fail closed: with [`DenyAllProvider`] installed the resend is `500` and no
/// mail is sent — never a silent success.
#[tokio::test]
async fn provider_unavailable_fails_resend_closed() {
    let _lock = PROVIDER_LOCK.lock().await;
    install_user_provider(Arc::new(DenyAllProvider));
    let mailer = array_mailer();
    let _signer = test_signer();
    let guard = guard_with_user_a();
    let session = session_for_user_a(&guard).await;

    let (status, _, body) = call(
        app_with_guard(Arc::clone(&guard)),
        post_with_session(RESEND_URI, "", &session),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "an unavailable provider must fail closed; body: {body}"
    );
    assert_eq!(mailer.count(), 0, "a fail-closed resend must not send mail");
}
