//! Password-confirmation tests (AUTH-011).
//!
//! * **Positive** — a correct password `303`s and the *same* cookie then passes
//!   the `password.confirm` gate (the previously-blocked `/settings/security`
//!   becomes reachable).
//! * **Negative** — a wrong password is `422` and the gate still blocks.
//! * **Auth gate** — an unauthenticated POST is `302` → `/login`.
//! * **Staleness** — a confirmation stamped beyond `AUTH_PASSWORD_TIMEOUT`
//!   (here the epoch) re-blocks the gate.

use axum::body::Body;
use axum::http::{header, StatusCode};

use super::super::settings_flows::PROVIDER_LOCK;
use super::super::{app, call, csrf_same_origin, method_request};
use super::{
    app_with_guard, field_errors, get_with_session, guard_with_user_a, post_with_session,
    session_for_user_a, verified_provider, CONFIRM_URI, SECURITY_URI, USER_A_PASSWORD,
};

/// Positive: a correct password `303`s and the **same** cookie then passes the
/// `password.confirm` gate (the previously-blocked page becomes reachable).
#[tokio::test]
async fn correct_password_confirms_and_unblocks_the_gate() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _provider = verified_provider();
    let guard = guard_with_user_a();
    let session = session_for_user_a(&guard).await;

    // Before: the gate blocks and redirects to the confirm screen.
    let (before, headers, _) = call(
        app_with_guard(std::sync::Arc::clone(&guard)),
        get_with_session(SECURITY_URI, &session),
    )
    .await;
    assert_eq!(before, StatusCode::FOUND);
    assert_eq!(
        headers.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some(CONFIRM_URI)
    );

    // Confirm with the correct password.
    let (status, _, body) = call(
        app_with_guard(std::sync::Arc::clone(&guard)),
        post_with_session(
            CONFIRM_URI,
            &format!("password={USER_A_PASSWORD}"),
            &session,
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::SEE_OTHER,
        "a correct password must confirm; body: {body}"
    );

    // After: the SAME cookie now passes the gate.
    let (after, _, after_body) = call(
        app_with_guard(std::sync::Arc::clone(&guard)),
        get_with_session(SECURITY_URI, &session),
    )
    .await;
    assert_eq!(
        after,
        StatusCode::OK,
        "the confirmed session must pass the gate"
    );
    assert!(after_body.contains("Security settings"));
}

/// Negative: a wrong password is `422` with a `password` field error and the
/// gate still blocks the page.
#[tokio::test]
async fn wrong_password_is_rejected_and_gate_still_blocks() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _provider = verified_provider();
    let guard = guard_with_user_a();
    let session = session_for_user_a(&guard).await;

    let (status, _, body) = call(
        app_with_guard(std::sync::Arc::clone(&guard)),
        post_with_session(CONFIRM_URI, "password=not-the-password", &session),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a wrong password must be rejected; body: {body}"
    );
    assert!(
        !field_errors(&body, "password").is_empty(),
        "a `password` field error is required; body: {body}"
    );

    let (after, headers, _) = call(
        app_with_guard(std::sync::Arc::clone(&guard)),
        get_with_session(SECURITY_URI, &session),
    )
    .await;
    assert_eq!(after, StatusCode::FOUND);
    assert_eq!(
        headers.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some(CONFIRM_URI),
        "a rejected confirmation must not open the gate"
    );
}

/// Negative: an unauthenticated confirmation is `302` → `/login`.
#[tokio::test]
async fn unauthenticated_confirmation_redirects_to_login() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _provider = verified_provider();

    let mut request = method_request("POST", CONFIRM_URI);
    *request.body_mut() = Body::from(format!("password={USER_A_PASSWORD}"));
    let (status, headers, _) = call(app(), csrf_same_origin(request)).await;

    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        headers.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some("/login")
    );
}

/// Negative: a confirmation stamped at the epoch (far beyond any timeout) is
/// stale and re-blocks the gate.
#[tokio::test]
async fn stale_confirmation_is_rejected_by_the_gate() {
    let _lock = PROVIDER_LOCK.lock().await;
    let _provider = verified_provider();
    let guard = guard_with_user_a();
    let session = session_for_user_a(&guard).await;
    // Stamp a confirmation that is older than the window.
    guard
        .confirm_password_at(&session, 0)
        .await
        .expect("write the stale timestamp");

    let (status, headers, _) = call(
        app_with_guard(std::sync::Arc::clone(&guard)),
        get_with_session(SECURITY_URI, &session),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        headers.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some(CONFIRM_URI),
        "a stale confirmation must be rejected"
    );
}
