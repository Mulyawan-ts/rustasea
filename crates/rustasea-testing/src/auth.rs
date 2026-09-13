//! Reusable auth test helpers (feature `auth`).
//!
//! Rust adaptation of laravel/livewire-starter-kit's `tests/TestCase.php`
//! conventions: the kit exposes `actingAs($user)` (authenticate a request),
//! `assertAuthenticated()` / `assertGuest()` (assert the session state), and
//! feature-flag gates read from `config/fortify.php`. This module mirrors
//! those conveniences over the real [`SessionGuard`] API so feature tests do
//! not re-implement the wiring in every suite.
//!
//! - [`login_as`] wires a credential lookup into a [`SessionGuard`] and issues
//!   a session [`Token`] (the `actingAs` analogue).
//! - [`assert_authenticated`] resolves the token back to an [`AuthUser`],
//!   panicking with a descriptive message on failure (Rust test convention).
//! - [`assert_guest`] asserts the token no longer authenticates.
//! - [`fortify_feature_enabled`] reads the real [`FortifyConfig`] feature
//!   toggles, mirroring Fortify's `Features::enabled()` checks.
//!
//! Enable with the `auth` cargo feature.

use std::sync::Arc;

use rustasea_auth::users::UserLookup;
use rustasea_auth::{AuthError, AuthUser, Credentials, FortifyConfig, Guard, SessionGuard, Token};

/// Built-in Fortify features, mirroring Laravel Fortify's `Features` list.
///
/// Each variant maps onto a toggle inside [`FortifyConfig::features`]; see
/// [`fortify_feature_enabled`] for the exact field read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FortifyFeature {
    /// User registration (`Features::registration`).
    Registration,
    /// Password resets (`Features::resetPasswords`).
    ResetPasswords,
    /// Email verification (`Features::emailVerification`).
    EmailVerification,
    /// Two-factor authentication (`Features::twoFactorAuthentication`).
    TwoFactorAuthentication,
    /// Passkeys / WebAuthn (`Features::passkeys`).
    Passkeys,
}

/// Whether `feature` is enabled in `config`.
///
/// Reads the real [`FortifyConfig::features`] fields:
///
/// - [`FortifyFeature::Registration`] → `features.registration`
/// - [`FortifyFeature::ResetPasswords`] → `features.reset_passwords`
/// - [`FortifyFeature::EmailVerification`] → `features.email_verification`
/// - [`FortifyFeature::TwoFactorAuthentication`] → always `true`
/// - [`FortifyFeature::Passkeys`] → always `true`
///
/// Two-factor authentication and passkeys are modelled as always-present
/// nested configs (`FortifyTwoFactorConfig` / `FortifyPasskeyFeatureConfig`)
/// with no enable/disable flag — mirroring Laravel Fortify, where a feature is
/// enabled by its presence in the `features` array. RustaSea's parity model
/// therefore treats both as enabled; their inner keys only tune behaviour
/// (`confirm`, `confirm_password`, `window`).
pub fn fortify_feature_enabled(config: &FortifyConfig, feature: FortifyFeature) -> bool {
    let features = &config.features;
    match feature {
        FortifyFeature::Registration => features.registration,
        FortifyFeature::ResetPasswords => features.reset_passwords,
        FortifyFeature::EmailVerification => features.email_verification,
        // No disable toggle exists in the parity config: presence = enabled.
        FortifyFeature::TwoFactorAuthentication | FortifyFeature::Passkeys => true,
    }
}

/// Authenticate against `guard` and return the issued session [`Token`].
///
/// The `actingAs` analogue: attaches `lookup` (typically an
/// [`Arc<MemoryUserRegistry>`](rustasea_auth::users::MemoryUserRegistry)) to
/// `guard` via [`SessionGuard::with_lookup`] and performs a real
/// `Guard::login` with `email`/`password`. The guard is consumed by the
/// builder call — capture `guard.session_store()` first when a later assertion
/// needs the same store (see the module tests).
///
/// # Errors
///
/// Propagates [`AuthError::BadCredentials`] for an unknown email or a wrong
/// password (indistinguishable, by design), and the store errors a login can
/// surface.
pub async fn login_as(
    guard: SessionGuard,
    lookup: Arc<dyn UserLookup>,
    email: &str,
    password: &str,
) -> Result<Token, AuthError> {
    let guard = guard.with_lookup(lookup);
    let credentials = Credentials {
        email: email.to_string(),
        password: password.to_string(),
    };
    guard.login(&credentials).await
}

/// Assert that `token` authenticates and return the resolved [`AuthUser`].
///
/// Wraps `Guard::parse`; panics with a descriptive message when the token is
/// invalid, expired, or revoked — the Rust test convention of failing loudly
/// inside the assertion helper (mirrors `assertAuthenticated()`). Accepts any
/// `Guard` (session, JWT, or custom), including a trait object.
///
/// # Panics
///
/// Panics when `guard.parse(token)` returns an error.
pub async fn assert_authenticated<G>(guard: &G, token: &str) -> AuthUser
where
    G: Guard + ?Sized,
{
    guard
        .parse(token)
        .await
        .unwrap_or_else(|err| panic!("expected an authenticated session, but parse failed: {err}"))
}

/// Assert that `token` is a guest (no longer authenticates) and return `true`.
///
/// Wraps `Guard::parse`; panics when the token still resolves to a user, so the
/// helper is a real assertion (mirrors `assertGuest()`) that also doubles as a
/// boolean predicate for `assert!(...)`. Accepts any `Guard`.
///
/// # Panics
///
/// Panics when `guard.parse(token)` succeeds (the token is still authenticated).
pub async fn assert_guest<G>(guard: &G, token: &str) -> bool
where
    G: Guard + ?Sized,
{
    match guard.parse(token).await {
        Ok(user) => panic!(
            "expected a guest session, but the token authenticated as {:?}",
            user.id
        ),
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use rustasea_auth::users::{AuthUserRecord, MemoryUserRegistry};
    use rustasea_auth::verify::{Argon2Verifier, PasswordVerifier};
    use rustasea_auth::SessionPolicy;

    /// Plaintext password seeded into the registry.
    const PASSWORD: &str = "s3cr3tPass";
    /// Login email of the seeded user.
    const EMAIL: &str = "ada@example.com";

    /// Seed a registry with one user whose password is really Argon2-hashed.
    fn registry() -> Arc<MemoryUserRegistry> {
        let hash = Argon2Verifier::new()
            .hash(PASSWORD)
            .expect("argon2 hashing succeeds");
        let registry = Arc::new(MemoryUserRegistry::default());
        registry.seed(AuthUserRecord {
            id: "user-1".into(),
            email: EMAIL.into(),
            password_hash: hash,
            email_verified_at: None,
        });
        registry
    }

    /// `login_as` issues a `Session` token that `assert_authenticated` resolves.
    #[tokio::test]
    async fn login_as_then_assert_authenticated_round_trips() {
        let policy = SessionPolicy::default();
        let guard = SessionGuard::new(policy.clone());
        // Keep the store so a second guard can parse the token after `login_as`
        // consumed the first one (both guards share the same MemoryStore).
        let store = guard.session_store();

        let token = login_as(guard, registry(), EMAIL, PASSWORD)
            .await
            .expect("valid credentials log in");
        assert_eq!(token.token_type, "Session");

        let guard = SessionGuard::with_store(policy, store);
        let user = assert_authenticated(&guard, &token.access_token).await;
        assert_eq!(user.id, "user-1");
        assert_eq!(user.email.as_deref(), Some(EMAIL));
        assert_eq!(user.guard, "session");
    }

    /// A wrong password is rejected with `BadCredentials` (no panic).
    #[tokio::test]
    async fn login_as_rejects_bad_credentials() {
        let guard = SessionGuard::new(SessionPolicy::default());
        let err = login_as(guard, registry(), EMAIL, "not-the-password")
            .await
            .expect_err("wrong password is rejected");
        assert_eq!(err, AuthError::BadCredentials);
    }

    /// `assert_guest` holds after `logout` and panics while still authenticated.
    #[tokio::test]
    async fn assert_guest_after_logout() {
        let policy = SessionPolicy::default();
        let guard = SessionGuard::new(policy.clone());
        let store = guard.session_store();

        let token = login_as(guard, registry(), EMAIL, PASSWORD)
            .await
            .expect("login succeeds");
        let guard = SessionGuard::with_store(policy, store);

        guard
            .logout(&token.access_token)
            .await
            .expect("logout succeeds");
        assert!(assert_guest(&guard, &token.access_token).await);
    }

    /// The three simple toggles default to enabled; disabling flips them off.
    #[tokio::test]
    async fn fortify_feature_gate_reads_boolean_toggles() {
        let mut config = FortifyConfig::default();
        assert!(fortify_feature_enabled(
            &config,
            FortifyFeature::Registration
        ));
        assert!(fortify_feature_enabled(
            &config,
            FortifyFeature::ResetPasswords
        ));
        assert!(fortify_feature_enabled(
            &config,
            FortifyFeature::EmailVerification
        ));

        config.features.registration = false;
        config.features.reset_passwords = false;
        config.features.email_verification = false;
        assert!(!fortify_feature_enabled(
            &config,
            FortifyFeature::Registration
        ));
        assert!(!fortify_feature_enabled(
            &config,
            FortifyFeature::ResetPasswords
        ));
        assert!(!fortify_feature_enabled(
            &config,
            FortifyFeature::EmailVerification
        ));
    }

    /// Two-factor and passkeys have no disable toggle, so they stay enabled.
    #[tokio::test]
    async fn fortify_feature_gate_structured_features_always_enabled() {
        let config = FortifyConfig::default();
        assert!(fortify_feature_enabled(
            &config,
            FortifyFeature::TwoFactorAuthentication
        ));
        assert!(fortify_feature_enabled(&config, FortifyFeature::Passkeys));
    }
}
