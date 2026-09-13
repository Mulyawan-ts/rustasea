/// Named rate-limiter registry — Laravel `RateLimiter::for()` parity.
///
/// The raw [`RateLimiter`] is *key-first*: callers derive a bucket key and ask
/// for a hit. The kit instead registers *named* limiters once at boot
/// (`FortifyServiceProvider::configureRateLimiting()`) and routes resolve them
/// by name at request time. This module supplies that registry: a map of
/// `name -> LimiterDefinition`, where each definition carries both the
/// [`Limit`] and a key-derivation closure over a borrowed [`LimiterInput`].
///
/// # Lifecycle contract
///
/// * **Register at boot, read at request time.** Interior mutability
///   ([`RwLock`]) keeps [`RateLimiterRegistry::register`] available without an
///   exclusive `&mut`, but the intended lifecycle is: populate the registry in
///   the service provider, then only call [`RateLimiterRegistry::check`] /
///   [`RateLimiterRegistry::for_name`] afterwards.
/// * **No global.** Per ADR-0007 the registry is an owned value passed
///   explicitly (typically behind an `Arc`), never a process-wide singleton.
/// * **Fail closed.** An unknown limiter name is a typed
///   [`LimiterError::UnknownLimiter`]; a definition whose key function declines
///   to derive a key is denied for the full window. Neither path is ever a
///   silent allow.
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use thiserror::Error;

use super::{Limit, RateLimiter, ThrottleDecision};

/// Borrowed request-ish input from which a named limiter derives its bucket key.
///
/// The fields mirror the identity sources the kit's limiters consult: the
/// submitted username, the peer IP, the session id (authenticated or pending
/// two-factor), and the WebAuthn credential id. Every field is optional so a
/// definition can express which identity it requires; a definition that needs a
/// field it was not given declines to derive a key (fail closed).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LimiterInput<'a> {
    /// Submitted username / email (the kit's `$username`).
    pub username: Option<&'a str>,
    /// Peer IP address (already resolved against trusted proxies by the caller).
    pub ip: Option<&'a str>,
    /// Session id — the authenticated session, or the pending two-factor
    /// session (`session()->get('login.id')`).
    pub session_id: Option<&'a str>,
    /// WebAuthn credential id, when the request carries one.
    pub credential_id: Option<&'a str>,
}

impl<'a> LimiterInput<'a> {
    /// Create an empty input (every identity absent).
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach the submitted username / email.
    pub fn with_username(mut self, username: &'a str) -> Self {
        self.username = Some(username);
        self
    }

    /// Attach the peer IP address.
    pub fn with_ip(mut self, ip: &'a str) -> Self {
        self.ip = Some(ip);
        self
    }

    /// Attach the session id (authenticated or pending two-factor).
    pub fn with_session_id(mut self, session_id: &'a str) -> Self {
        self.session_id = Some(session_id);
        self
    }

    /// Attach the WebAuthn credential id.
    pub fn with_credential_id(mut self, credential_id: &'a str) -> Self {
        self.credential_id = Some(credential_id);
        self
    }
}

/// Object-safe key-derivation closure stored by a [`LimiterDefinition`].
///
/// `Send + Sync` so the definition can live in a provider or an `Arc`; higher
/// ranked over the input lifetime so one closure serves every borrow. `None`
/// means "identity unavailable" and is treated as fail closed by
/// [`RateLimiterRegistry::check`].
pub type KeyDeriver = dyn for<'a> Fn(&LimiterInput<'a>) -> Option<String> + Send + Sync;

/// A named limiter: the [`Limit`] plus the closure deriving its bucket key.
///
/// Mirrors the shape of Laravel's `RateLimiter::for('login', fn (Request $r) =>
/// Limit::perMinute(5)->by(...))` — the closure captures the key strategy and
/// the [`Limit`] captures the threshold. Cheap to clone: the closure is shared
/// through an [`Arc`].
#[derive(Clone)]
pub struct LimiterDefinition {
    /// Threshold and window for this limiter.
    pub limit: Limit,
    /// Key-derivation closure (object-safe, `Send + Sync`).
    key: Arc<KeyDeriver>,
}

impl LimiterDefinition {
    /// Build a definition from a limit and a key-derivation closure.
    pub fn new<F>(limit: Limit, key: F) -> Self
    where
        F: for<'a> Fn(&LimiterInput<'a>) -> Option<String> + Send + Sync + 'static,
    {
        Self {
            limit,
            key: Arc::new(key),
        }
    }

    /// Derive the bucket key for `input`, or `None` when the identity is absent.
    pub fn derive_key(&self, input: &LimiterInput<'_>) -> Option<String> {
        (self.key)(input)
    }
}

impl std::fmt::Debug for LimiterDefinition {
    /// Manual debug — the boxed key closure has no `Debug` impl.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LimiterDefinition")
            .field("limit", &self.limit)
            .field("key", &"<key fn>")
            .finish()
    }
}

/// Typed error raised while resolving or applying a named limiter.
///
/// A dedicated type (rather than a new [`crate::error::ThrottleError`] variant)
/// keeps the throttle error surface free of duplicates — `ThrottleError` is
/// owned by `error.rs`, which this task must not edit.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum LimiterError {
    /// No limiter is registered under the requested name (fail closed).
    #[error("unknown rate limiter {0:?}")]
    UnknownLimiter(String),
}

/// Registry of named limiters consulted at request time.
///
/// Owns the shared [`RateLimiter`] backend so a single call
/// ([`RateLimiterRegistry::check`]) resolves a name, derives its key, and
/// records one hit. Definitions live behind an [`RwLock`] to honour the
/// "register at boot, read at request time" contract without an exclusive
/// borrow.
pub struct RateLimiterRegistry {
    /// Shared backend the definitions hit.
    limiter: Arc<dyn RateLimiter>,
    /// Named definitions (`name -> definition`).
    definitions: RwLock<HashMap<String, LimiterDefinition>>,
}

impl RateLimiterRegistry {
    /// Create an empty registry over a shared limiter backend.
    pub fn new(limiter: Arc<dyn RateLimiter>) -> Self {
        Self {
            limiter,
            definitions: RwLock::new(HashMap::new()),
        }
    }

    /// Register (or replace) the definition stored under `name`.
    ///
    /// Returns the previous definition when the name was already taken, like
    /// `HashMap::insert`. A poisoned lock is a no-op returning `None` — the
    /// registry never panics on a poisoned lock.
    pub fn register(
        &self,
        name: impl Into<String>,
        definition: LimiterDefinition,
    ) -> Option<LimiterDefinition> {
        self.definitions
            .write()
            .ok()
            .and_then(|mut map| map.insert(name.into(), definition))
    }

    /// Look up the definition registered under `name`.
    ///
    /// Returns an owned clone (cheap — the key closure is shared through an
    /// [`Arc`]) rather than a reference, because the definition lives behind a
    /// lock guard that cannot outlive the call. Laravel's `RateLimiter::for`
    /// accessor maps onto this method.
    pub fn for_name(&self, name: &str) -> Option<LimiterDefinition> {
        self.definitions
            .read()
            .ok()
            .and_then(|map| map.get(name).cloned())
    }

    /// Whether a limiter is registered under `name`.
    pub fn contains(&self, name: &str) -> bool {
        self.definitions
            .read()
            .map(|map| map.contains_key(name))
            .unwrap_or(false)
    }

    /// Number of registered limiters.
    pub fn len(&self) -> usize {
        self.definitions
            .read()
            .map(|map| map.len())
            .unwrap_or_default()
    }

    /// Whether the registry holds no limiters.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The registered limiter names, sorted for deterministic iteration.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .definitions
            .read()
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default();
        names.sort();
        names
    }

    /// Derive the bucket key for `name` without recording a hit.
    ///
    /// `Ok(Some(key))` when the definition produced a key, `Ok(None)` when the
    /// definition declined (identity absent — the caller should fail closed),
    /// and `Err` for an unknown limiter name.
    ///
    /// # Errors
    ///
    /// [`LimiterError::UnknownLimiter`] when `name` is not registered.
    pub fn key_for(
        &self,
        name: &str,
        input: &LimiterInput<'_>,
    ) -> Result<Option<String>, LimiterError> {
        let definition = self
            .for_name(name)
            .ok_or_else(|| LimiterError::UnknownLimiter(name.to_string()))?;
        Ok(definition.derive_key(input))
    }

    /// Resolve `name`, derive its key, and record one hit.
    ///
    /// The convenience the kit's actions call after building a
    /// [`LimiterInput`]. An unknown name is a typed error (never an allow); a
    /// definition that cannot derive a key is denied for its full window.
    ///
    /// # Errors
    ///
    /// [`LimiterError::UnknownLimiter`] when `name` is not registered.
    pub fn check(
        &self,
        name: &str,
        input: &LimiterInput<'_>,
    ) -> Result<ThrottleDecision, LimiterError> {
        let definition = self
            .for_name(name)
            .ok_or_else(|| LimiterError::UnknownLimiter(name.to_string()))?;
        match definition.derive_key(input) {
            Some(key) => Ok(self.limiter.hit(&key, &definition.limit)),
            // No identity to bucket on — fail closed for the whole window
            // rather than letting every such request share one "unlimited" key.
            None => Ok(ThrottleDecision::Denied {
                retry_after_secs: definition.limit.decay_secs,
            }),
        }
    }
}

impl std::fmt::Debug for RateLimiterRegistry {
    /// Manual debug — the definitions hold boxed closures with no `Debug`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RateLimiterRegistry")
            .field("driver", &self.limiter.driver())
            .field("limiters", &self.names())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::throttle::limiter::MemoryRateLimiter;

    fn registry() -> (Arc<MemoryRateLimiter>, RateLimiterRegistry) {
        let limiter = Arc::new(MemoryRateLimiter::new());
        limiter.set_now(1_000_000);
        let registry = RateLimiterRegistry::new(limiter.clone());
        (limiter, registry)
    }

    fn always(key: &'static str, limit: Limit) -> LimiterDefinition {
        LimiterDefinition::new(limit, move |_input| Some(key.to_string()))
    }

    /// A registered limiter allows up to the threshold, then denies.
    #[test]
    fn registered_limiter_allows_then_denies() {
        let (_limiter, registry) = registry();
        registry.register("login", always("k", Limit::per_minute(2)));
        let input = LimiterInput::new();
        assert_eq!(
            registry.check("login", &input),
            Ok(ThrottleDecision::Allowed { remaining: 1 })
        );
        assert_eq!(
            registry.check("login", &input),
            Ok(ThrottleDecision::Allowed { remaining: 0 })
        );
        match registry.check("login", &input) {
            Ok(ThrottleDecision::Denied { retry_after_secs }) => {
                assert!((1..=60).contains(&retry_after_secs));
            }
            other => panic!("expected denial, got {other:?}"),
        }
    }

    /// An unknown limiter name is a typed error, never an allow.
    #[test]
    fn unknown_limiter_is_typed_error() {
        let (_limiter, registry) = registry();
        let error = registry
            .check("missing", &LimiterInput::new())
            .expect_err("unknown limiter must error");
        assert_eq!(error, LimiterError::UnknownLimiter("missing".to_string()));
        assert!(registry.for_name("missing").is_none());
        assert!(registry.key_for("missing", &LimiterInput::new()).is_err());
    }

    /// A definition that declines to derive a key is denied (fail closed).
    #[test]
    fn unavailable_key_fails_closed() {
        let (_limiter, registry) = registry();
        registry.register(
            "two-factor",
            LimiterDefinition::new(Limit::per_minute(5), |_i| None),
        );
        match registry.check("two-factor", &LimiterInput::new()) {
            Ok(ThrottleDecision::Denied { retry_after_secs }) => assert_eq!(retry_after_secs, 60),
            other => panic!("expected fail-closed denial, got {other:?}"),
        }
    }

    /// Re-registering a name replaces the previous definition.
    #[test]
    fn register_replaces_and_reports_previous() {
        let (_limiter, registry) = registry();
        assert!(registry
            .register("x", always("a", Limit::per_minute(1)))
            .is_none());
        let replaced = registry.register("x", always("b", Limit::per_minute(2)));
        assert!(replaced.is_some());
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.names(), vec!["x".to_string()]);
    }
}
