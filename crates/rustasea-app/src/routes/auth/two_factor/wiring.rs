//! Process-wide wiring for the two-factor flow (AUTH-016).
//!
//! Everything here resolves a shared, process-wide dependency once — the
//! persistence store, the at-rest cipher, the issuer name, and the
//! `two-factor` limiter registry — with a **fail-closed** default: a value
//! that cannot be resolved yields `None`/a deny, never a permissive guess.
//!
//! # Store default
//!
//! The default store is an **empty** [`MemoryTwoFactorStore`], so a fresh app
//! has no enrolled users and an ordinary login is never interrupted. A real
//! deployment injects a database-backed [`TwoFactorStore`] through
//! [`install_two_factor_store`]; [`DenyAllTwoFactorStore`] remains available as
//! an explicit lockdown.
//!
//! # Cipher
//!
//! The cipher is derived from `app.key` ([`SecretCipher::from_app_key`]); a
//! blank/missing key yields `None`, so enabling two-factor answers `500` rather
//! than sealing a secret under a default key. Tests inject a deterministic
//! cipher with [`install_two_factor_cipher`].
//!
//! # Recovery-code display
//!
//! Recovery codes are stored **hashed** (single-use, authoritative). Because a
//! one-way hash cannot be shown again, the plaintext set is cached here for the
//! GET view, populated on enable/regenerate and cleared on disable. A restart
//! empties the cache, so the view then answers `404` and the user regenerates.
//!
//! [`DenyAllTwoFactorStore`]: rustasea::auth::two_factor::DenyAllTwoFactorStore
//! [`SecretCipher::from_app_key`]: rustasea::auth::two_factor::SecretCipher::from_app_key

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use rustasea::auth::two_factor::{
    MemoryTwoFactorStore, SecretCipher, TwoFactorService, TwoFactorStore, DEFAULT_WINDOW,
};
use rustasea::auth::{two_factor_definition, MemoryRateLimiter, RateLimiterRegistry, TWO_FACTOR};
use rustasea::foundation::AppConfig;
use rustasea::ConfigLoader;

use crate::routes::helpers::fortify_config;

/// Default issuer shown in authenticator apps when `app.name` is unavailable.
const DEFAULT_ISSUER: &str = "RustaSea";

/// Process-wide [`TwoFactorStore`] cell, defaulting to an empty in-memory store.
fn store_cell() -> &'static RwLock<Arc<dyn TwoFactorStore>> {
    static STORE: OnceLock<RwLock<Arc<dyn TwoFactorStore>>> = OnceLock::new();
    STORE.get_or_init(|| RwLock::new(Arc::new(MemoryTwoFactorStore::default())))
}

/// Resolve the shared [`TwoFactorStore`] seam.
pub(super) fn two_factor_store() -> Arc<dyn TwoFactorStore> {
    match store_cell().read() {
        Ok(store) => Arc::clone(&store),
        Err(_) => Arc::new(MemoryTwoFactorStore::default()),
    }
}

/// Test-only override of the process-wide [`TwoFactorStore`] seam.
#[cfg(test)]
pub(crate) fn install_two_factor_store(store: Arc<dyn TwoFactorStore>) {
    if let Ok(mut slot) = store_cell().write() {
        *slot = store;
    }
}

/// Process-wide test cipher override cell; see [`install_two_factor_cipher`].
#[cfg(test)]
fn cipher_override() -> &'static RwLock<Option<SecretCipher>> {
    static OVERRIDE: OnceLock<RwLock<Option<SecretCipher>>> = OnceLock::new();
    OVERRIDE.get_or_init(|| RwLock::new(None))
}

/// Install (or clear) the test cipher override.
#[cfg(test)]
pub(crate) fn install_two_factor_cipher(cipher: Option<SecretCipher>) {
    if let Ok(mut slot) = cipher_override().write() {
        *slot = cipher;
    }
}

/// Reset every process-wide two-factor seam (tests serialize via the lock).
#[cfg(test)]
pub(crate) fn reset_two_factor_wiring() {
    install_two_factor_store(Arc::new(MemoryTwoFactorStore::default()));
    install_two_factor_cipher(None);
    if let Ok(mut cache) = recovery_cache().write() {
        cache.clear();
    }
}

/// Resolve the process-wide [`SecretCipher`] from `app.key`, fail-closed.
///
/// A blank or missing key yields `None`; the caller answers `500` rather than
/// sealing a secret with a default key.
pub(super) fn two_factor_cipher() -> Option<SecretCipher> {
    #[cfg(test)]
    if let Ok(slot) = cipher_override().read() {
        if let Some(cipher) = slot.as_ref() {
            return Some(cipher.clone());
        }
    }
    static BUILT: OnceLock<Option<SecretCipher>> = OnceLock::new();
    BUILT
        .get_or_init(|| {
            ConfigLoader::load()
                .ok()
                .and_then(|loader| AppConfig::from_loader(&loader).ok())
                .and_then(|config| {
                    config
                        .key
                        .as_deref()
                        .and_then(|key| SecretCipher::from_app_key(key).ok())
                })
        })
        .clone()
}

/// Resolve the authenticator issuer name from `app.name`.
pub(super) fn issuer() -> String {
    ConfigLoader::load()
        .ok()
        .and_then(|loader| AppConfig::from_loader(&loader).ok())
        .map(|config| config.name)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_ISSUER.to_string())
}

/// Build the two-factor service; `None` when no cipher key is configured.
///
/// The verification window comes from `[fortify.features.two_factor_authentication].window`
/// and defaults to [`DEFAULT_WINDOW`] (one step either side of "now").
pub(super) fn two_factor_service() -> Option<TwoFactorService> {
    let cipher = two_factor_cipher()?;
    let window = fortify_config()
        .features
        .two_factor_authentication
        .window
        .unwrap_or(u64::from(DEFAULT_WINDOW)) as u32;
    Some(TwoFactorService::new(
        two_factor_store(),
        cipher,
        issuer(),
        window,
    ))
}

/// Process-wide plaintext recovery-code cache, keyed by user id.
fn recovery_cache() -> &'static RwLock<HashMap<String, Vec<String>>> {
    static CACHE: OnceLock<RwLock<HashMap<String, Vec<String>>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Cache the freshly minted plaintext recovery codes for the GET view.
pub(super) fn store_recovery_codes(user_id: &str, codes: Vec<String>) {
    if let Ok(mut cache) = recovery_cache().write() {
        cache.insert(user_id.to_string(), codes);
    }
}

/// Read the cached plaintext recovery codes, if the set is still displayable.
pub(super) fn cached_recovery_codes(user_id: &str) -> Option<Vec<String>> {
    recovery_cache().read().ok()?.get(user_id).cloned()
}

/// Drop the cached recovery codes (disable, or a consumed set).
pub(super) fn clear_recovery_codes(user_id: &str) {
    if let Ok(mut cache) = recovery_cache().write() {
        cache.remove(user_id);
    }
}

/// Process-wide `two-factor` limiter registry (built once).
///
/// The shipped `[fortify.limiters].two_factor` is unset, so the shared login
/// registry does not register it; this dedicated registry registers the kit's
/// `two-factor` definition (5/minute, keyed on the pending session id) once, and
/// is process-wide so its window accumulates across requests.
pub(super) fn two_factor_registry() -> &'static RateLimiterRegistry {
    static REGISTRY: OnceLock<RateLimiterRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let registry = RateLimiterRegistry::new(Arc::new(MemoryRateLimiter::new()));
        registry.register(TWO_FACTOR, two_factor_definition());
        registry
    })
}
