/// The kit's three named rate limiters, as constructor helpers.
///
/// Source of truth: `FortifyServiceProvider::configureRateLimiting()` in the
/// Laravel kit, which registers:
///
/// | name | limit | key |
/// |---|---|---|
/// | `login` | `perMinute(5)` | `Str::transliterate(Str::lower($username)).'|'.$request->ip()` |
/// | `two-factor` | `perMinute(5)` | the pending-2FA session id (`session()->get('login.id')`) |
/// | `passkeys` | `perMinute(10)` | `($credential.id ?: $request->session()->getId()).'|'.$request->ip()` |
///
/// Each helper returns a [`LimiterDefinition`] carrying both the [`Limit`] and
/// the kit-equivalent key derivation. [`RateLimiterRegistry::from_fortify_config`]
/// registers exactly the limiters the `[fortify.limiters]` table names.
use std::sync::Arc;

use crate::config::FortifyLimiterConfig;
use crate::throttle::limiter::MemoryRateLimiter;
use crate::throttle::registry::{LimiterDefinition, LimiterInput, RateLimiterRegistry};
use crate::throttle::{Limit, RateLimiter};

/// Attempts per minute for the `login` and `two-factor` limiters (kit parity).
pub const LOGIN_MAX_ATTEMPTS: u32 = 5;

/// Attempts per minute for the `passkeys` limiter (kit parity).
pub const PASSKEYS_MAX_ATTEMPTS: u32 = 10;

/// Canonical kit limiter name for the login route.
pub const LOGIN: &str = "login";

/// Canonical kit limiter name for the two-factor challenge route.
pub const TWO_FACTOR: &str = "two-factor";

/// Canonical kit limiter name for the passkey routes.
pub const PASSKEYS: &str = "passkeys";

/// Normalize a username for the `login` bucket key.
///
/// Mirrors the *observable* effect of the kit's
/// `Str::transliterate(Str::lower($username))` for ASCII input: surrounding
/// whitespace is trimmed and the remainder is lowercased (Unicode-aware, via
/// `str::to_lowercase`). Full Unicode transliteration — folding accents such as
/// `é -> e` — is **out of scope** and deliberately not attempted: doing it
/// correctly needs a Unicode database, and the kit's `Str::transliterate` is a
/// best-effort ASCII fold, not a normative identity transform. Two usernames
/// that differ only by case or surrounding whitespace therefore share a bucket,
/// which is the property the throttle relies on.
pub fn normalize_username(username: &str) -> String {
    username.trim().to_lowercase()
}

/// Build the `login` limiter: `perMinute(5)`, key `normalized-username|ip`.
///
/// The username is normalized by [`normalize_username`]; the IP is appended
/// verbatim. A missing username declines to derive a key (fail closed); a
/// missing IP is represented by an empty segment, which still yields a finite
/// per-username bucket.
pub fn login_definition() -> LimiterDefinition {
    LimiterDefinition::new(
        Limit::per_minute(LOGIN_MAX_ATTEMPTS),
        |input: &LimiterInput<'_>| {
            let username = input.username?;
            let ip = input.ip.unwrap_or_default();
            Some(format!("{}|{ip}", normalize_username(username)))
        },
    )
}

/// Build the `two-factor` limiter: `perMinute(5)`, keyed on the pending-2FA
/// session id alone (no IP suffix — the kit mirrors the challenge identity).
///
/// The user is not yet authenticated at challenge time, so the only stable
/// identity is the pending-login session id. A missing session id declines to
/// derive a key (fail closed).
pub fn two_factor_definition() -> LimiterDefinition {
    LimiterDefinition::new(
        Limit::per_minute(LOGIN_MAX_ATTEMPTS),
        |input: &LimiterInput<'_>| input.session_id.map(str::to_string),
    )
}

/// Build the `passkeys` limiter: `perMinute(10)`, key
/// `(credential_id | session_id)|ip`.
///
/// The credential id is preferred when present, falling back to the session id,
/// matching the kit's `($credential.id ?: $request->session()->getId())`. A
/// request carrying neither declines to derive a key (fail closed).
pub fn passkeys_definition() -> LimiterDefinition {
    LimiterDefinition::new(
        Limit::per_minute(PASSKEYS_MAX_ATTEMPTS),
        |input: &LimiterInput<'_>| {
            let identity = input.credential_id.or(input.session_id)?;
            let ip = input.ip.unwrap_or_default();
            Some(format!("{identity}|{ip}"))
        },
    )
}

/// The `(name, definition)` pairs the kit's service provider registers.
///
/// Used by [`RateLimiterRegistry::from_fortify_config`]; exposed so callers can
/// register the canonical names without a config file.
pub fn kit_limiters() -> [(String, LimiterDefinition); 3] {
    [
        (LOGIN.to_string(), login_definition()),
        (TWO_FACTOR.to_string(), two_factor_definition()),
        (PASSKEYS.to_string(), passkeys_definition()),
    ]
}

impl RateLimiterRegistry {
    /// Build a registry over a fresh in-memory limiter, registering exactly the
    /// limiters named by `[fortify.limiters]`.
    ///
    /// The kit ships `login = "login"` and leaves `two-factor` / `passkeys`
    /// commented out; registration is therefore driven by *presence* in the
    /// config, never hardcoded. Concretely:
    ///
    /// * `login` — the built-in [`login_definition`] is registered under
    ///   `config.login` when that name is non-blank.
    /// * `two-factor` — registered under `config.two_factor` only when it is
    ///   `Some`; the shipped TOML comments it out, so it stays unregistered.
    /// * `passkeys` — registered under `config.passkeys` only when it is `Some`.
    ///
    /// A blank configured name is treated as "not configured" (fail closed:
    /// the route naming it will resolve to [`crate::throttle::registry::LimiterError::UnknownLimiter`]).
    pub fn from_fortify_config(config: &FortifyLimiterConfig) -> Self {
        Self::from_fortify_config_with_limiter(config, Arc::new(MemoryRateLimiter::new()))
    }

    /// Like [`RateLimiterRegistry::from_fortify_config`], but over a caller-
    /// supplied backend (e.g. a shared limiter or a Redis-backed one).
    pub fn from_fortify_config_with_limiter(
        config: &FortifyLimiterConfig,
        limiter: Arc<dyn RateLimiter>,
    ) -> Self {
        let registry = Self::new(limiter);
        register_configured(&registry, &config.login, login_definition());
        if let Some(name) = config.two_factor.as_deref() {
            register_configured(&registry, name, two_factor_definition());
        }
        if let Some(name) = config.passkeys.as_deref() {
            register_configured(&registry, name, passkeys_definition());
        }
        registry
    }
}

/// Register `definition` under `name` unless `name` is blank.
///
/// A blank name means "the config did not enable this limiter", so nothing is
/// registered — the caller fails closed on the unknown name rather than the
/// blank entry silently matching every lookup.
fn register_configured(registry: &RateLimiterRegistry, name: &str, definition: LimiterDefinition) {
    if !name.trim().is_empty() {
        registry.register(name.trim().to_string(), definition);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::throttle::ThrottleDecision;

    /// The login key folds case and trims, and appends the IP.
    #[test]
    fn login_key_normalizes_username() {
        let def = login_definition();
        let a = def.derive_key(
            &LimiterInput::new()
                .with_username("ADA@Example.com")
                .with_ip("1.2.3.4"),
        );
        let b = def.derive_key(
            &LimiterInput::new()
                .with_username("ada@example.com")
                .with_ip("1.2.3.4"),
        );
        assert_eq!(a, Some("ada@example.com|1.2.3.4".to_string()));
        assert_eq!(a, b);
    }

    /// A missing username declines to derive a key.
    #[test]
    fn login_key_requires_username() {
        let def = login_definition();
        assert_eq!(
            def.derive_key(&LimiterInput::new().with_ip("1.2.3.4")),
            None
        );
    }

    /// The two-factor key is the session id with no IP suffix.
    #[test]
    fn two_factor_key_is_session_only() {
        let def = two_factor_definition();
        assert_eq!(
            def.derive_key(
                &LimiterInput::new()
                    .with_session_id("pending-1")
                    .with_ip("1.2.3.4")
            ),
            Some("pending-1".to_string())
        );
        assert_eq!(
            def.derive_key(&LimiterInput::new().with_ip("1.2.3.4")),
            None
        );
    }

    /// The passkeys key prefers the credential id and falls back to the session.
    #[test]
    fn passkeys_key_prefers_credential() {
        let def = passkeys_definition();
        let input = LimiterInput::new()
            .with_credential_id("cred-1")
            .with_session_id("sess-1")
            .with_ip("1.2.3.4");
        assert_eq!(def.derive_key(&input), Some("cred-1|1.2.3.4".to_string()));

        let fallback = LimiterInput::new()
            .with_session_id("sess-1")
            .with_ip("1.2.3.4");
        assert_eq!(
            def.derive_key(&fallback),
            Some("sess-1|1.2.3.4".to_string())
        );

        assert_eq!(
            def.derive_key(&LimiterInput::new().with_ip("1.2.3.4")),
            None
        );
    }

    /// Only the limiters named by the config are registered.
    #[test]
    fn config_drives_registration() {
        // Shipped default: only `login` named; `two-factor` / `passkeys` absent.
        let default = FortifyLimiterConfig::default();
        let registry = RateLimiterRegistry::from_fortify_config(&default);
        assert!(registry.contains("login"));
        assert!(!registry.contains("two-factor"));
        assert!(!registry.contains("passkeys"));

        let full = FortifyLimiterConfig {
            login: "login".to_string(),
            two_factor: Some("two-factor".to_string()),
            passkeys: Some("passkeys".to_string()),
        };
        let registry = RateLimiterRegistry::from_fortify_config(&full);
        assert_eq!(
            registry.names(),
            vec![
                "login".to_string(),
                "passkeys".to_string(),
                "two-factor".to_string(),
            ]
        );
    }

    /// A configured limiter enforces its kit threshold end to end.
    #[test]
    fn configured_login_limiter_denies_sixth_attempt() {
        let registry = RateLimiterRegistry::from_fortify_config(&FortifyLimiterConfig::default());
        let input = LimiterInput::new()
            .with_username("ada@example.com")
            .with_ip("1.2.3.4");
        for _ in 0..5 {
            assert!(matches!(
                registry.check("login", &input),
                Ok(ThrottleDecision::Allowed { .. })
            ));
        }
        assert!(matches!(
            registry.check("login", &input),
            Ok(ThrottleDecision::Denied { .. })
        ));
    }
}
