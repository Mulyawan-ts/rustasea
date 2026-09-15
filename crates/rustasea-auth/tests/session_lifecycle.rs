//! Session lifecycle integration test over the public `SessionGuard` API.
//!
//! Rust adaptation of laravel/livewire-starter-kit's
//! `tests/Feature/Auth/AuthenticationTest.php`, which asserts the
//! login/logout/guest flows over HTTP. RustaSea has no HTTP auth endpoint yet,
//! so the same lifecycle is exercised against the public guard surface:
//! `login` → `parse` (the "authenticated" assertion), `logout` → `parse`
//! (the "guest" assertion), plus refresh rotation and the
//! `loginUsingId` gate.
//!
//! Only public APIs (`rustasea_auth::...`) are used: a real
//! [`Argon2Verifier`] hashes the seed password, a [`MemoryUserRegistry`]
//! supplies the credential lookup, and the guard runs over the default
//! in-memory session store.

use std::sync::Arc;

use rustasea_auth::users::{AuthUserRecord, MemoryUserRegistry};
use rustasea_auth::verify::{Argon2Verifier, PasswordVerifier};
use rustasea_auth::{AuthError, Credentials, Guard, SessionGuard, SessionPolicy};

/// Plaintext password seeded into the registry.
const PASSWORD: &str = "s3cr3tPass";
/// UUID of the seeded user.
const USER_ID: &str = "user-1";
/// Login email of the seeded user.
const EMAIL: &str = "ada@example.com";

/// Build a guard over an in-memory store with one seeded user and the default
/// Argon2 verifier, so wrong passwords are genuinely rejected.
fn seeded_guard() -> SessionGuard {
    let hash = Argon2Verifier::new()
        .hash(PASSWORD)
        .expect("argon2 hashing succeeds");
    let registry = Arc::new(MemoryUserRegistry::default());
    registry.seed(AuthUserRecord {
        id: USER_ID.into(),
        email: EMAIL.into(),
        password_hash: hash,
        email_verified_at: None,
        timezone: None,
    });
    SessionGuard::new(SessionPolicy::default()).with_lookup(registry)
}

/// Credentials for the seeded user with an arbitrary password.
fn credentials(password: &str) -> Credentials {
    Credentials {
        email: EMAIL.into(),
        password: password.into(),
    }
}

/// login mints a `Session` token and `parse` round-trips the identity.
#[tokio::test]
async fn login_then_parse_round_trips_the_identity() {
    let guard = seeded_guard();

    let token = guard
        .login(&credentials(PASSWORD))
        .await
        .expect("valid credentials log in");
    assert_eq!(token.token_type, "Session");

    let principal = guard
        .parse(&token.access_token)
        .await
        .expect("issued token parses back");
    assert_eq!(principal.id, USER_ID);
    assert_eq!(principal.email.as_deref(), Some(EMAIL));
    assert_eq!(principal.guard, "session");
}

/// A wrong password and an unknown email surface the same error variant, so an
/// attacker cannot enumerate accounts by probing responses.
#[tokio::test]
async fn bad_password_and_unknown_email_are_indistinguishable() {
    let guard = seeded_guard();

    let wrong_password = guard
        .login(&credentials("not-the-password"))
        .await
        .expect_err("wrong password is rejected");
    let unknown_email = guard
        .login(&Credentials {
            email: "nobody@example.com".into(),
            password: PASSWORD.into(),
        })
        .await
        .expect_err("unknown email is rejected");

    assert_eq!(wrong_password, AuthError::BadCredentials);
    assert_eq!(unknown_email, AuthError::BadCredentials);
    assert_eq!(wrong_password, unknown_email);
}

/// Logout destroys the session, so replaying the token is rejected — the
/// "guest" assertion of the starter kit.
#[tokio::test]
async fn logout_invalidates_the_token() {
    let guard = seeded_guard();
    let token = guard
        .login(&credentials(PASSWORD))
        .await
        .expect("valid credentials log in");

    guard
        .logout(&token.access_token)
        .await
        .expect("logout succeeds");

    let err = guard
        .parse(&token.access_token)
        .await
        .expect_err("replayed token is rejected");
    assert_eq!(err, AuthError::InvalidToken);
}

/// Refresh rotates the session id while retaining the identity, and the old
/// id is no longer accepted.
#[tokio::test]
async fn refresh_rotates_the_session_id_and_keeps_the_identity() {
    let guard = seeded_guard();
    let token = guard
        .login(&credentials(PASSWORD))
        .await
        .expect("valid credentials log in");

    let rotated = guard
        .refresh(&token.access_token)
        .await
        .expect("refresh succeeds");
    assert_ne!(rotated.access_token, token.access_token);

    let principal = guard
        .parse(&rotated.access_token)
        .await
        .expect("rotated token parses");
    assert_eq!(principal.id, USER_ID);
    assert_eq!(principal.guard, "session");

    let err = guard
        .parse(&token.access_token)
        .await
        .expect_err("old token is rejected after rotation");
    assert_eq!(err, AuthError::InvalidToken);
}

/// `loginUsingId` is fail-closed by default and only works once explicitly
/// enabled.
#[tokio::test]
async fn login_using_id_is_disabled_by_default_then_enabled() {
    let disabled = seeded_guard();
    let err = disabled
        .login_using_id(USER_ID)
        .await
        .expect_err("loginUsingId is disabled by default");
    assert!(matches!(err, AuthError::Disabled(_)));

    let enabled = seeded_guard().with_allow_login_using_id(true);
    let token = enabled
        .login_using_id(USER_ID)
        .await
        .expect("loginUsingId works once enabled");
    assert_eq!(token.token_type, "Session");

    let principal = enabled
        .parse(&token.access_token)
        .await
        .expect("loginUsingId token parses");
    assert_eq!(principal.id, USER_ID);
    assert_eq!(principal.guard, "session");
}

/// Each login mints a distinct session id (session-fixation defense).
#[tokio::test]
async fn repeated_logins_mint_distinct_tokens() {
    let guard = seeded_guard();

    let first = guard
        .login(&credentials(PASSWORD))
        .await
        .expect("first login succeeds");
    let second = guard
        .login(&credentials(PASSWORD))
        .await
        .expect("second login succeeds");

    assert_ne!(first.access_token, second.access_token);
    assert_eq!(first.refresh_token, first.access_token);
    assert_eq!(second.refresh_token, second.access_token);

    // Both live sessions still resolve to the same identity.
    let a = guard
        .parse(&first.access_token)
        .await
        .expect("first token parses");
    let b = guard
        .parse(&second.access_token)
        .await
        .expect("second token parses");
    assert_eq!(a.id, USER_ID);
    assert_eq!(b.id, USER_ID);
}
