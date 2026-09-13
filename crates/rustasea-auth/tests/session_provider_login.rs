//! Integration tests for the async database-backed login path (AUTH-004).
//!
//! Covers [`SessionGuard::login_with_provider`]: it authenticates against an
//! async [`UserProvider`], mints a fresh session token, and preserves the
//! account-enumeration resistance of the sync `Guard::login` path. Provider
//! write/read behaviour itself is covered in `user_provider.rs`.

use std::sync::Arc;

use rustasea_auth::users::{DenyAllProvider, MemoryUserProvider, NewUserRecord, UserProvider};
use rustasea_auth::verify::{Argon2Verifier, PasswordVerifier};
use rustasea_auth::{AuthError, Credentials, Guard, SessionGuard, SessionPolicy};

/// Real argon2 hasher/verifier (production KDF, no test double).
fn hasher() -> Argon2Verifier {
    Argon2Verifier::new()
}

/// A fresh guard over an in-memory store, wired with the real verifier.
fn guard() -> SessionGuard {
    SessionGuard::new(SessionPolicy::default()).with_verifier(Arc::new(hasher()))
}

/// Credentials for the seeded `ada@example.com` user.
fn creds(password: &str) -> Credentials {
    Credentials {
        email: "ada@example.com".into(),
        password: password.into(),
    }
}

/// Seed `ada@example.com` with an argon2 hash of `password`.
async fn provider_with_ada(password: &str) -> MemoryUserProvider {
    let provider = MemoryUserProvider::default();
    provider
        .create(NewUserRecord::new(
            "Ada",
            "ada@example.com",
            hasher().hash(password).expect("hash"),
        ))
        .await
        .expect("create");
    provider
}

/// `login_with_provider` issues a session token that `parse` accepts, carrying
/// the identity (and the verification state) through the session.
#[tokio::test]
async fn login_with_provider_issues_parseable_token() {
    let provider = provider_with_ada("s3cr3tPass").await;
    let ada = provider
        .find_by_email("ada@example.com")
        .await
        .expect("find")
        .expect("present");
    provider
        .set_email_verified_at(&ada.id, Some("2026-01-01T00:00:00Z"))
        .await
        .expect("verify");

    let guard = guard();
    let token = guard
        .login_with_provider(&provider, &creds("s3cr3tPass"))
        .await
        .expect("login succeeds");
    assert_eq!(token.token_type, "Session");

    let principal = guard
        .parse(&token.access_token)
        .await
        .expect("parse accepts the issued token");
    assert_eq!(principal.id, ada.id);
    assert_eq!(principal.email.as_deref(), Some("ada@example.com"));
    assert_eq!(principal.guard, "session");
    assert!(principal.is_email_verified());
}

/// `update_password` then a login with the new password succeeds while the old
/// password is rejected.
#[tokio::test]
async fn update_password_then_login_with_new_password() {
    let provider = provider_with_ada("old-password").await;
    let ada = provider
        .find_by_email("ada@example.com")
        .await
        .expect("find")
        .expect("present");

    let new_hash = hasher().hash("new-password").expect("hash");
    provider
        .update_password(&ada.id, &new_hash)
        .await
        .expect("update_password succeeds");

    let guard = guard();
    let token = guard
        .login_with_provider(&provider, &creds("new-password"))
        .await
        .expect("login with new password succeeds");
    assert_eq!(token.token_type, "Session");

    let err = guard
        .login_with_provider(&provider, &creds("old-password"))
        .await
        .expect_err("old password is rejected");
    assert_eq!(err, AuthError::BadCredentials);
}

/// Unknown email and wrong password both yield an indistinguishable
/// `BadCredentials` (account-enumeration resistance).
#[tokio::test]
async fn unknown_email_and_wrong_password_are_indistinguishable() {
    let provider = provider_with_ada("s3cr3tPass").await;
    let guard = guard();

    let unknown = guard
        .login_with_provider(
            &provider,
            &Credentials {
                email: "ghost@example.com".into(),
                password: "s3cr3tPass".into(),
            },
        )
        .await
        .expect_err("unknown email rejected");
    let wrong = guard
        .login_with_provider(&provider, &creds("wrong-password"))
        .await
        .expect_err("wrong password rejected");

    assert_eq!(unknown, AuthError::BadCredentials);
    assert_eq!(wrong, AuthError::BadCredentials);
    // Byte-for-byte identical: no channel leaks which case occurred.
    assert_eq!(unknown, wrong);
    assert_eq!(unknown.to_string(), wrong.to_string());
    assert_eq!(unknown.code(), wrong.code());
}

/// An empty-password credential is rejected with `BadCredentials` (fail-closed),
/// never a panic.
#[tokio::test]
async fn empty_password_is_rejected() {
    let provider = provider_with_ada("s3cr3tPass").await;
    let guard = guard();
    let err = guard
        .login_with_provider(&provider, &creds(""))
        .await
        .expect_err("empty password rejected");
    assert_eq!(err, AuthError::BadCredentials);
}

/// Every `login_with_provider` mints a distinct session id (session-fixation
/// defense), exactly like the sync `Guard::login` path.
#[tokio::test]
async fn login_with_provider_rotates_session_id_each_time() {
    let provider = provider_with_ada("s3cr3tPass").await;
    let guard = guard();
    let first = guard
        .login_with_provider(&provider, &creds("s3cr3tPass"))
        .await
        .expect("first login");
    let second = guard
        .login_with_provider(&provider, &creds("s3cr3tPass"))
        .await
        .expect("second login");
    assert_ne!(first.access_token, second.access_token);
    // Both tokens parse to the same identity under distinct ids.
    let a = guard.parse(&first.access_token).await.expect("parse first");
    let b = guard
        .parse(&second.access_token)
        .await
        .expect("parse second");
    assert_eq!(a.id, b.id);
}

/// An email change clears verification, and the cleared state round-trips
/// through a fresh `login_with_provider` + `parse`.
#[tokio::test]
async fn email_change_reverification_round_trips_through_login() {
    let provider = provider_with_ada("s3cr3tPass").await;
    let ada = provider
        .find_by_email("ada@example.com")
        .await
        .expect("find")
        .expect("present");
    provider
        .set_email_verified_at(&ada.id, Some("2026-01-01T00:00:00Z"))
        .await
        .expect("verify");
    // Email change → force re-verification.
    provider
        .set_email_verified_at(&ada.id, None)
        .await
        .expect("clear");

    let guard = guard();
    let token = guard
        .login_with_provider(&provider, &creds("s3cr3tPass"))
        .await
        .expect("login");
    let principal = guard.parse(&token.access_token).await.expect("parse");
    assert!(!principal.is_email_verified());
    assert_eq!(principal.email_verified_at, None);
}

/// Two distinct users resolve to their own identities through one guard, and
/// the provider is usable behind `&dyn UserProvider` (object safety).
#[tokio::test]
async fn provider_is_object_safe_and_isolates_users() {
    let provider = MemoryUserProvider::default();
    let verifier = hasher();
    let ada = provider
        .create(NewUserRecord::new(
            "Ada",
            "ada@example.com",
            verifier.hash("ada-pass").expect("hash"),
        ))
        .await
        .expect("create ada");
    let grace = provider
        .create(NewUserRecord::new(
            "Grace",
            "grace@example.com",
            verifier.hash("grace-pass").expect("hash"),
        ))
        .await
        .expect("create grace");
    assert_ne!(ada.id, grace.id);

    // Exercised through a trait object, proving object safety.
    let dyn_provider: &dyn UserProvider = &provider;
    let guard = guard();
    let token = guard
        .login_with_provider(
            dyn_provider,
            &Credentials {
                email: "grace@example.com".into(),
                password: "grace-pass".into(),
            },
        )
        .await
        .expect("grace login");
    let principal = guard.parse(&token.access_token).await.expect("parse");
    assert_eq!(principal.id, grace.id);
}

/// A `DenyAllProvider` login is denied with a typed error, not a panic.
#[tokio::test]
async fn deny_all_provider_login_is_typed_error() {
    let guard = guard();
    let err = guard
        .login_with_provider(&DenyAllProvider, &creds("s3cr3tPass"))
        .await
        .expect_err("deny-all rejects login");
    assert!(matches!(err, AuthError::Disabled(_)));
}
