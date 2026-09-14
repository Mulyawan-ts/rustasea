//! Process-wide wiring for the passkey / WebAuthn flow (AUTH-017).
//!
//! Everything here resolves a shared, process-wide dependency once — the
//! credential store, the challenge store, the resolved relying-party settings,
//! and the `passkeys` limiter registry — with a **fail-closed** default: a value
//! that cannot be resolved yields `None`/a deny, never a permissive guess.
//!
//! # Store defaults
//!
//! The default credential store is an **empty** [`MemoryPasskeyStore`], so a
//! fresh app has no enrolled credentials and an ordinary login is never
//! interrupted. A real deployment injects a database-backed [`PasskeyStore`]
//! through [`install_passkey_store`]; [`DenyAllPasskeyStore`] remains available
//! as an explicit lockdown. The default challenge store is an empty
//! [`MemoryChallengeStore`] so a ceremony never resolves a stale challenge.
//!
//! # Relying party
//!
//! [`passkey_service`] derives `relying_party_id`, `allowed_origins`,
//! `user_handle_secret`, and `timeout` from
//! [`FortifyConfig::resolve_passkeys`](rustasea::auth::FortifyConfig::resolve_passkeys)
//! over `app.url`/`app.key`, and the human-readable relying-party name from
//! `app.name`. A config failure yields `None` (the caller answers `500`).
//!
//! [`DenyAllPasskeyStore`]: rustasea::auth::passkeys::DenyAllPasskeyStore

use std::sync::{Arc, OnceLock, RwLock};

use rustasea::auth::{
    passkeys_definition, ChallengeStore, MemoryChallengeStore, MemoryPasskeyStore,
    MemoryRateLimiter, PasskeyService, PasskeyStore, RateLimiterRegistry, PASSKEYS,
};
use rustasea::foundation::AppConfig;
use rustasea::ConfigLoader;

use crate::routes::helpers::fortify_config;

/// Default relying-party name shown to authenticators when `app.name` is unset.
const DEFAULT_RP_NAME: &str = "RustaSea";

/// Process-wide [`PasskeyStore`] cell, defaulting to an empty in-memory store.
fn store_cell() -> &'static RwLock<Arc<dyn PasskeyStore>> {
    static STORE: OnceLock<RwLock<Arc<dyn PasskeyStore>>> = OnceLock::new();
    STORE.get_or_init(|| RwLock::new(Arc::new(MemoryPasskeyStore::default())))
}

/// Resolve the shared [`PasskeyStore`] seam.
pub(super) fn passkey_store() -> Arc<dyn PasskeyStore> {
    match store_cell().read() {
        Ok(store) => Arc::clone(&store),
        Err(_) => Arc::new(MemoryPasskeyStore::default()),
    }
}

/// Test-only override of the process-wide [`PasskeyStore`] seam.
#[cfg(test)]
pub(crate) fn install_passkey_store(store: Arc<dyn PasskeyStore>) {
    if let Ok(mut slot) = store_cell().write() {
        *slot = store;
    }
}

/// Process-wide [`ChallengeStore`] cell, defaulting to an empty in-memory store.
fn challenge_cell() -> &'static RwLock<Arc<dyn ChallengeStore>> {
    static CELL: OnceLock<RwLock<Arc<dyn ChallengeStore>>> = OnceLock::new();
    CELL.get_or_init(|| RwLock::new(Arc::new(MemoryChallengeStore::default())))
}

/// Resolve the shared [`ChallengeStore`] seam.
pub(super) fn challenge_store() -> Arc<dyn ChallengeStore> {
    match challenge_cell().read() {
        Ok(cell) => Arc::clone(&cell),
        Err(_) => Arc::new(MemoryChallengeStore::default()),
    }
}

/// Test-only override of the process-wide [`ChallengeStore`] seam.
#[cfg(test)]
pub(crate) fn install_challenge_store(store: Arc<dyn ChallengeStore>) {
    if let Ok(mut slot) = challenge_cell().write() {
        *slot = store;
    }
}

/// Reset every process-wide passkey seam (tests serialize via the lock).
#[cfg(test)]
pub(crate) fn reset_passkey_wiring() {
    install_passkey_store(Arc::new(MemoryPasskeyStore::default()));
    install_challenge_store(Arc::new(MemoryChallengeStore::default()));
}

/// Load the application config, or `None` when it cannot be resolved.
fn app_config() -> Option<AppConfig> {
    ConfigLoader::load()
        .ok()
        .and_then(|loader| AppConfig::from_loader(&loader).ok())
}

/// Build the passkey service from the process-wide seams, fail-closed.
///
/// The relying-party settings come from `[fortify]` resolved over `app.url` and
/// `app.key`; the relying-party name from `app.name` (falling back to
/// [`DEFAULT_RP_NAME`]). A missing config yields `None` so the caller answers
/// `500` rather than running a ceremony with a guessed relying party.
pub(super) fn passkey_service() -> Option<PasskeyService> {
    let config = app_config()?;
    let name = if config.name.trim().is_empty() {
        DEFAULT_RP_NAME.to_string()
    } else {
        config.name.clone()
    };
    let resolved = fortify_config().resolve_passkeys(&config);
    Some(PasskeyService::new(
        passkey_store(),
        challenge_store(),
        resolved,
        name,
    ))
}

/// Process-wide `passkeys` limiter registry (built once).
///
/// The shipped `[fortify.limiters].passkeys` is unset, so the shared login
/// registry does not register it; this dedicated registry registers the kit's
/// `passkeys` definition (keyed on the credential id, falling back to the
/// session id, plus the peer ip) once, and is process-wide so its window
/// accumulates across requests.
pub(super) fn passkeys_registry() -> &'static RateLimiterRegistry {
    static REGISTRY: OnceLock<RateLimiterRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let registry = RateLimiterRegistry::new(Arc::new(MemoryRateLimiter::new()));
        registry.register(PASSKEYS, passkeys_definition());
        registry
    })
}
