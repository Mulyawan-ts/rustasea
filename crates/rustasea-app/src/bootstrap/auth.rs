//! Auth wiring — the RustaSea analogue of `FortifyServiceProvider`.
//!
//! This module builds the real [`SessionGuard`] from `config/session.toml`,
//! installs it into the foundation [`Container`] so the boot DAG can hand it to
//! [`crate::bootstrap::providers::AuthServiceProvider`], and exposes the
//! container key the provider resolves.
//!
//! # Fail-closed posture
//!
//! There is no database-backed `UserLookup` yet. The guard is therefore built
//! over [`StaticLookup`], which yields **no** credentials: every `login` fails
//! with `AuthError::BadCredentials` and no principal is ever minted. This is
//! deliberate — the wiring is real (config → guard → state → middleware), but
//! credential resolution stays shut until a DB-backed lookup lands. A missing
//! or malformed `session.toml` also fails closed: [`build_session_guard`]
//! returns `None` and the app serves without an auth slot rather than inventing
//! a permissive default.
//!
//! # User provider seam
//!
//! Alongside the guard this module wires the async [`UserProvider`] seam under
//! [`USER_PROVIDER_KEY`] (see [`build_user_provider`] /
//! [`install_user_provider`]). It is **fail-closed by default**: unless the
//! dev-only `RUSTASEA_DEV_SEED_USER` env var is set, the app installs
//! [`DenyAllProvider`], which resolves no credentials and refuses every write.
//! The guard and provider are installed together by [`install_session_guard`].
//!
//! [`SessionGuard`]: rustasea::auth::SessionGuard
//! [`UserProvider`]: rustasea::auth::UserProvider
//! [`DenyAllProvider`]: rustasea::auth::DenyAllProvider
//! [`Container`]: rustasea::foundation::Container
//! [`StaticLookup`]: rustasea::auth::users::StaticLookup

use std::sync::Arc;

use rustasea::auth::users::AuthUserRecord;
use rustasea::auth::verify::{Argon2Verifier, PasswordVerifier};
use rustasea::auth::{DenyAllProvider, MemoryUserProvider, SessionGuard, UserProvider};
use rustasea::config::ConfigLoader;
use rustasea::foundation::{Container, CONFIG_LOADER_KEY};

/// Container key under which the built [`SessionGuard`] is installed.
///
/// [`crate::bootstrap::providers::AuthServiceProvider`] writes the guard here
/// during `register`; `main`/tests resolve it during `boot` to seed
/// [`AppState`](rustasea::http::AppState).
pub const SESSION_GUARD_KEY: &str = "auth.session_guard";

/// Container key under which the built [`UserProvider`] is installed.
///
/// The provider is the async write+read seam for registration, password reset,
/// email verification, and DB-backed login. It is installed together with the
/// session guard by [`install_session_guard`] and resolved by [`user_provider`].
///
/// [`UserProvider`]: rustasea::auth::UserProvider
pub const USER_PROVIDER_KEY: &str = "auth.user_provider";

/// Build the session guard from the layered `config/session.toml`.
///
/// Loads the config through [`ConfigLoader::load`], deserializes the `[session]`
/// table with [`SessionConfig::from_loader`] (which applies the `SESSION_*`
/// environment overrides and validates the driver), and constructs a
/// [`SessionGuard`] over the in-memory store via
/// [`SessionGuard::from_config`].
///
/// The guard is built over the fail-closed [`StaticLookup`](rustasea::auth::users::StaticLookup)
/// (the `from_config` default): no user is resolvable yet, so a `login` always
/// fails closed. A real DB-backed `UserLookup` replaces it here later.
///
/// # Returns
///
/// `None` when the config directory is unreadable, the `[session]` table is
/// malformed, or it selects an unimplemented driver — the app then runs
/// unauthenticated rather than panicking. A warning is printed so the operator
/// sees why auth is disabled.
///
/// [`SessionConfig::from_loader`]: rustasea::auth::SessionConfig::from_loader
pub fn build_session_guard() -> Option<SessionGuard> {
    match ConfigLoader::load() {
        Ok(loader) => build_session_guard_from(&loader),
        Err(error) => {
            eprintln!("auth: config load failed, session auth disabled: {error}");
            None
        }
    }
}

/// Build the session guard from an already-loaded layered [`ConfigLoader`].
///
/// [`install_session_guard`] passes the container-mounted loader here so the
/// guard and the rest of the app share one configuration source. Deserializes
/// the `[session]` table with [`SessionConfig::from_loader`] (applying the
/// `SESSION_*` environment overrides and validating the driver) and constructs a
/// [`SessionGuard`] over the fail-closed [`StaticLookup`].
///
/// # Returns
///
/// `None` when the `[session]` table is malformed or selects an unimplemented
/// driver — the app then runs unauthenticated rather than panicking.
///
/// [`SessionConfig::from_loader`]: rustasea::auth::SessionConfig::from_loader
/// [`StaticLookup`]: rustasea::auth::users::StaticLookup
pub fn build_session_guard_from(loader: &ConfigLoader) -> Option<SessionGuard> {
    let session = match rustasea::auth::SessionConfig::from_loader(loader) {
        Ok(session) => session,
        Err(error) => {
            eprintln!("auth: invalid session config, session auth disabled: {error}");
            return None;
        }
    };
    match SessionGuard::from_config(&session) {
        Ok(guard) => Some(guard),
        Err(error) => {
            eprintln!("auth: session guard build failed, auth disabled: {error}");
            None
        }
    }
}

/// Build the guard and install it into `container` under [`SESSION_GUARD_KEY`].
///
/// This is the single wiring entry point: [`crate::bootstrap::providers::AuthServiceProvider`]
/// calls it during `register`, and [`crate::bootstrap::app`] calls it before
/// boot so `main` can resolve the guard to seed `AppState`.
///
/// It prefers the boot-time [`ConfigLoader`] mounted under
/// [`CONFIG_LOADER_KEY`] (see `rustasea_foundation::Application::boot`) and only
/// falls back to a direct [`ConfigLoader::load`] when the application was not
/// booted — so the guard reads the same configuration source as the rest of the
/// app.
///
/// # Returns
///
/// `true` when a guard was built and installed; `false` when the guard could
/// not be built (see [`build_session_guard`]) — in which case the container is
/// left untouched and the app fails closed.
pub fn install_session_guard(container: &mut Container) -> bool {
    // Install the user provider seam alongside the guard so both are wired
    // together in a single call. The provider is fail-closed (see
    // [`build_user_provider`]) and its install never fails.
    install_user_provider(container);
    // Prefer the boot-time loader mounted under `CONFIG_LOADER_KEY`; fall back
    // to a direct load when the app was not booted.
    let guard = match container
        .get::<Arc<ConfigLoader>>(CONFIG_LOADER_KEY)
        .cloned()
    {
        Some(loader) => build_session_guard_from(&loader),
        None => build_session_guard(),
    };
    match guard {
        Some(guard) => {
            container.instance(SESSION_GUARD_KEY, Arc::new(guard));
            true
        }
        None => false,
    }
}

/// Resolve the installed guard from `container`, if present.
///
/// The container stores an `Arc<SessionGuard>`; a `None` result means auth was
/// never wired (or wiring failed) and callers must fail closed.
pub fn session_guard(container: &Container) -> Option<Arc<SessionGuard>> {
    container
        .get::<Arc<SessionGuard>>(SESSION_GUARD_KEY)
        .cloned()
}

/// Dev-only env var that seeds a single in-memory user, as `email:password`.
///
/// When **unset** (the default, and the only safe production posture) the app
/// installs [`DenyAllProvider`]. See [`build_user_provider`].
const DEV_SEED_USER_ENV: &str = "RUSTASEA_DEV_SEED_USER";

/// Build the async [`UserProvider`] seam.
///
/// # ⚠️ Dev/test only — NEVER production, NEVER enabled by default
///
/// This is a **local development and test convenience**. It is **fail-closed by
/// default**: when the `RUSTASEA_DEV_SEED_USER` environment variable is unset,
/// the app installs [`DenyAllProvider`], which resolves no credentials and
/// refuses every write — no user can log in and nothing is persisted.
///
/// Only when `RUSTASEA_DEV_SEED_USER` is set **and** in the form
/// `email:password` (split on the **first** `:`; the password may itself
/// contain colons) does this build a [`MemoryUserProvider`] seeded with that
/// single user. The password is hashed with [`Argon2Verifier`] before seeding,
/// so the plaintext is never stored.
///
/// The in-memory provider has **no durability, no cross-process coordination,
/// and no password policy** — it exists purely so local flows (registration,
/// reset, verification, DB-backed login) can be exercised without a database.
/// A production deployment must inject a real, database-backed `UserProvider`
/// here instead; leaving the env var set in production is a security defect.
///
/// The seeded record uses the email as its user id (no UUID generator is
/// reachable from this crate) and marks the account verified so the dev login
/// flow is not blocked by the `verified` gate.
///
/// # Returns
///
/// An `Arc<dyn UserProvider>` — never `None`. A malformed seed value or a
/// hashing failure prints a warning and falls back to [`DenyAllProvider`]
/// rather than panicking, preserving the fail-closed posture.
///
/// [`UserProvider`]: rustasea::auth::UserProvider
/// [`DenyAllProvider`]: rustasea::auth::DenyAllProvider
/// [`MemoryUserProvider`]: rustasea::auth::MemoryUserProvider
/// [`Argon2Verifier`]: rustasea::auth::verify::Argon2Verifier
pub fn build_user_provider() -> Arc<dyn UserProvider> {
    let raw = match std::env::var(DEV_SEED_USER_ENV) {
        Ok(raw) => raw,
        // Unset => fail closed. This is the default and the only safe
        // production posture.
        Err(_) => return Arc::new(DenyAllProvider),
    };

    // Split on the FIRST ':' only; the password keeps any further colons.
    let (email, password) = match raw.split_once(':') {
        Some((email, password)) if !email.is_empty() => (email, password),
        _ => {
            eprintln!(
                "auth: {DEV_SEED_USER_ENV} is malformed (expected email:password); \
                 installing DenyAllProvider"
            );
            return Arc::new(DenyAllProvider);
        }
    };

    let password_hash = match Argon2Verifier::new().hash(password) {
        Ok(password_hash) => password_hash,
        Err(error) => {
            eprintln!(
                "auth: failed to hash {DEV_SEED_USER_ENV} password ({error}); \
                 installing DenyAllProvider"
            );
            return Arc::new(DenyAllProvider);
        }
    };

    let provider = MemoryUserProvider::default();
    provider.seed(AuthUserRecord {
        id: email.to_string(),
        email: email.to_string(),
        password_hash,
        email_verified_at: Some("1970-01-01T00:00:00Z".to_string()),
        timezone: None,
    });
    Arc::new(provider)
}

/// Build the provider and install it into `container` under [`USER_PROVIDER_KEY`].
///
/// Mirrors [`install_session_guard`]: it stores the built value as a container
/// instance. [`install_session_guard`] calls this so the guard and the provider
/// are always installed together.
///
/// # Returns
///
/// Always `true` — [`build_user_provider`] is total and never fails (a bad seed
/// falls back to [`DenyAllProvider`]).
///
/// [`DenyAllProvider`]: rustasea::auth::DenyAllProvider
pub fn install_user_provider(container: &mut Container) -> bool {
    container.instance(USER_PROVIDER_KEY, build_user_provider());
    true
}

/// Resolve the installed [`UserProvider`] from `container`, if present.
///
/// The container stores an `Arc<dyn UserProvider>`; a `None` result means the
/// provider was never wired and callers must fail closed. (An un-wired app
/// still installs [`DenyAllProvider`] through [`install_session_guard`], so a
/// `None` here means the guard install never ran at all.)
///
/// [`UserProvider`]: rustasea::auth::UserProvider
/// [`DenyAllProvider`]: rustasea::auth::DenyAllProvider
// Allowed dead code: this is the provider-side analogue of [`session_guard`],
// the documented resolution seam for downstream callers. Nothing in this binary
// consumes it yet (the guard path is still the only wired consumer), and the
// callers that will (`main`/handlers) live outside this module, which this
// change is scoped to.
#[allow(dead_code)]
pub fn user_provider(container: &Container) -> Option<Arc<dyn UserProvider>> {
    container
        .get::<Arc<dyn UserProvider>>(USER_PROVIDER_KEY)
        .cloned()
}
