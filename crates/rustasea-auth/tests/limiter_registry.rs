//! End-to-end tests for the named rate-limiter registry (AUTH-007).
//!
//! Exercises the public API only: the kit's three limiters resolve by name with
//! kit-equivalent keys, the config drives which limiters are registered, and an
//! unknown name fails closed with a typed error rather than an allow.
use std::sync::Arc;

use rustasea_auth::throttle::defaults;
use rustasea_auth::{
    kit_limiters, login_definition, named_limiter_layer, passkeys_definition,
    two_factor_definition, FortifyLimiterConfig, LimiterDefinition, LimiterError, LimiterInput,
    MemoryRateLimiter, RateLimiterRegistry, ThrottleDecision,
};

/// Build a registry over a frozen-clock in-memory limiter.
fn registry() -> RateLimiterRegistry {
    let limiter = Arc::new(MemoryRateLimiter::new());
    limiter.set_now(1_000_000);
    RateLimiterRegistry::new(limiter)
}

/// Registering a limiter and checking it five times allows 5 then denies.
#[test]
fn login_allows_five_then_denies_with_retry_after() {
    let registry = registry();
    registry.register("login", login_definition());

    let input = LimiterInput::new()
        .with_username("ada@example.com")
        .with_ip("1.2.3.4");

    for attempt in 0..5 {
        assert_eq!(
            registry.check("login", &input),
            Ok(ThrottleDecision::Allowed {
                remaining: 4 - attempt
            }),
            "attempt {attempt} should be allowed"
        );
    }

    match registry.check("login", &input) {
        Ok(ThrottleDecision::Denied { retry_after_secs }) => {
            assert!((1..=60).contains(&retry_after_secs), "sane Retry-After");
        }
        other => panic!("6th attempt must be denied, got {other:?}"),
    }
}

/// The `login` key is case-insensitive and trims — same bucket for mixed case.
#[test]
fn login_key_is_case_insensitive_for_same_ip() {
    let registry = registry();
    registry.register("login", login_definition());

    let upper = LimiterInput::new()
        .with_username("ADA@Example.com")
        .with_ip("1.2.3.4");
    let lower = LimiterInput::new()
        .with_username("ada@example.com")
        .with_ip("1.2.3.4");

    assert_eq!(
        registry.key_for("login", &upper).expect("known limiter"),
        registry.key_for("login", &lower).expect("known limiter"),
        "mixed-case usernames share one bucket"
    );

    // Exhaust through the mixed-case spelling, then the lower-case spelling is
    // already denied — proving they share the bucket.
    for _ in 0..5 {
        assert!(matches!(
            registry.check("login", &upper),
            Ok(ThrottleDecision::Allowed { .. })
        ));
    }
    assert!(matches!(
        registry.check("login", &lower),
        Ok(ThrottleDecision::Denied { .. })
    ));
}

/// A different IP uses a different bucket for the same username.
#[test]
fn login_buckets_are_isolated_by_ip() {
    let registry = registry();
    registry.register("login", login_definition());

    let a = LimiterInput::new()
        .with_username("ada@example.com")
        .with_ip("1.2.3.4");
    let b = LimiterInput::new()
        .with_username("ada@example.com")
        .with_ip("5.6.7.8");

    for _ in 0..5 {
        assert!(matches!(
            registry.check("login", &a),
            Ok(ThrottleDecision::Allowed { .. })
        ));
    }
    assert!(matches!(
        registry.check("login", &a),
        Ok(ThrottleDecision::Denied { .. })
    ));
    // A different IP is unaffected.
    assert!(matches!(
        registry.check("login", &b),
        Ok(ThrottleDecision::Allowed { .. })
    ));
}

/// The `two-factor` key is the pending session id, with no IP suffix.
#[test]
fn two_factor_key_is_pending_session_id() {
    let definition = two_factor_definition();
    let key = definition
        .derive_key(
            &LimiterInput::new()
                .with_session_id("login-id-42")
                .with_ip("1.2.3.4"),
        )
        .expect("session id present");
    assert_eq!(key, "login-id-42", "no ip suffix for two-factor");
}

/// The `passkeys` key prefers `credential_id` and falls back to `session_id`.
#[test]
fn passkeys_key_prefers_credential_then_session() {
    let definition = passkeys_definition();

    let with_credential = definition
        .derive_key(
            &LimiterInput::new()
                .with_credential_id("cred-1")
                .with_session_id("sess-1")
                .with_ip("1.2.3.4"),
        )
        .expect("credential present");
    assert_eq!(with_credential, "cred-1|1.2.3.4");

    let session_only = definition
        .derive_key(
            &LimiterInput::new()
                .with_session_id("sess-1")
                .with_ip("1.2.3.4"),
        )
        .expect("session present");
    assert_eq!(session_only, "sess-1|1.2.3.4");
}

/// The `passkeys` limiter allows 10 then denies (kit threshold).
#[test]
fn passkeys_allows_ten_then_denies() {
    let registry = registry();
    registry.register("passkeys", passkeys_definition());
    let input = LimiterInput::new()
        .with_credential_id("cred-1")
        .with_ip("1.2.3.4");
    for _ in 0..10 {
        assert!(matches!(
            registry.check("passkeys", &input),
            Ok(ThrottleDecision::Allowed { .. })
        ));
    }
    assert!(matches!(
        registry.check("passkeys", &input),
        Ok(ThrottleDecision::Denied { .. })
    ));
}

/// An unknown limiter name is a typed error, never an allow.
#[test]
fn unknown_limiter_name_is_typed_error() {
    let registry = registry();
    let error = registry
        .check("nope", &LimiterInput::new())
        .expect_err("unknown name must not resolve");
    assert_eq!(error, LimiterError::UnknownLimiter("nope".to_string()));
    assert!(registry.for_name("nope").is_none());
}

/// A limiter that cannot derive a key is denied (fail closed), not allowed.
#[test]
fn definition_without_identity_fails_closed() {
    let registry = registry();
    registry.register("login", login_definition());
    // No username in the input -> the login definition declines a key.
    match registry.check("login", &LimiterInput::new().with_ip("1.2.3.4")) {
        Ok(ThrottleDecision::Denied { retry_after_secs }) => assert_eq!(retry_after_secs, 60),
        other => panic!("expected fail-closed denial, got {other:?}"),
    }
}

/// Registration is driven by config: an absent key is NOT registered.
#[test]
fn config_drives_which_limiters_register() {
    // Shipped default names only `login`.
    let default = FortifyLimiterConfig::default();
    let registry = RateLimiterRegistry::from_fortify_config(&default);
    assert!(registry.contains("login"));
    assert!(!registry.contains("two-factor"));
    assert!(!registry.contains("passkeys"));
    assert!(registry.check("two-factor", &LimiterInput::new()).is_err());

    // Enabling every limiter registers all three.
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
            "two-factor".to_string()
        ]
    );
}

/// `kit_limiters()` exposes the three canonical `(name, definition)` pairs.
#[test]
fn kit_limiters_lists_all_three() {
    let pairs = kit_limiters();
    let names: Vec<&str> = pairs.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, vec!["login", "two-factor", "passkeys"]);
    for (_, definition) in &pairs {
        // Every kit definition is a valid, cloneable definition.
        let _clone: LimiterDefinition = definition.clone();
    }
}

/// The route-binding helper returns a closure the router DSL can consume.
///
/// This asserts the *shape* only — the app crate wires it into
/// `rustasea_router::Router::register_middleware`, which `rustasea-auth` must
/// not depend on.
#[test]
fn named_limiter_layer_is_router_compatible() {
    let registry = Arc::new({
        let r = registry();
        r.register("login", login_definition());
        r
    });
    let apply = named_limiter_layer(registry, "login");
    // Compose it with a MethodRouter the way `register_middleware` will.
    let router = apply(axum::routing::get(|| async { "ok" }));
    let _: axum::routing::MethodRouter<()> = router;
}

/// The `defaults` submodule exposes the same constructors the re-exports do.
#[test]
fn defaults_module_is_reachable() {
    let _ = defaults::normalize_username("  ADA@Example.com ");
    assert_eq!(defaults::LOGIN, "login");
    assert_eq!(defaults::TWO_FACTOR, "two-factor");
    assert_eq!(defaults::PASSKEYS, "passkeys");
}
