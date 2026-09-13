//! End-to-end test for config-driven session guard wiring (CFG-002).
//!
//! Exercises the public API only: a typed [`SessionConfig`] parsed from a
//! temp `session.toml` builds a [`SessionGuard`] whose cookie, serialization
//! policy, and TTL match the config, and the shipped repository
//! `config/session.toml` parses into the same typed shape.
use std::path::PathBuf;

use rustasea_auth::config::SessionConfig;
use rustasea_auth::session::SessionGuard;
use rustasea_auth::{Guard, SessionCookieConfig};
use rustasea_config::ConfigLoader;
use tower_sessions::cookie::SameSite;

/// Path to the workspace-root `config/` directory.
fn workspace_config_dir() -> PathBuf {
    // `CARGO_MANIFEST_DIR` = `<workspace>/crates/rustasea-auth`; the workspace
    // root is two levels up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("config")
}

/// A typed config over the in-memory store carries the configured cookie and TTL.
#[tokio::test]
async fn session_guard_from_config_applies_cookie_policy_and_ttl() {
    let config = SessionConfig {
        lifetime_minutes: 45,
        secure: true,
        http_only: true,
        same_site: "strict".to_string(),
        path: "/app".to_string(),
        domain: Some("example.com".to_string()),
        ..SessionConfig::default()
    };

    let guard = SessionGuard::from_config(&config).expect("guard builds from config");
    assert_eq!(guard.ttl_secs(), 45 * 60);

    let cookie: &SessionCookieConfig = guard.cookie();
    assert_eq!(cookie.name, "rustasea-session");
    assert!(cookie.secure);
    assert!(cookie.http_only);
    assert_eq!(cookie.same_site, SameSite::Strict);
    assert_eq!(cookie.path, "/app");
    assert_eq!(cookie.domain.as_deref(), Some("example.com"));

    assert_eq!(guard.policy().serialization, "json");
    assert_eq!(guard.policy().prefix, "rustasea-session-");

    // Stateless contract: the guard exposes no per-request identity.
    assert!(guard.user().await.expect("user resolves").is_none());
}

/// `with_ttl` overrides only the advertised lifetime, keeping the const fallback
/// for guards built without config.
#[tokio::test]
async fn session_guard_with_ttl_overrides_lifetime() {
    let guard = SessionGuard::new(rustasea_auth::SessionPolicy::default()).with_ttl(60);
    assert_eq!(guard.ttl_secs(), 60);
    assert_eq!(
        guard.cookie().name,
        "rustasea-session",
        "TTL override must not disturb the cookie config"
    );
}

/// `with_config` applies the config policy but preserves an injected allow-list.
#[tokio::test]
async fn with_config_preserves_injected_allow_list() {
    let policy = rustasea_auth::SessionPolicy::with_classes(vec!["App::UserDto".to_string()]);
    let guard = SessionGuard::new(policy)
        .with_config(&SessionConfig::default())
        .expect("config applies");
    assert_eq!(guard.policy().prefix, "rustasea-session-");
    assert_eq!(
        guard.policy().serializable_classes,
        vec!["App::UserDto".to_string()],
        "an injected allow-list must survive config application"
    );
}

/// The real repository `config/session.toml` parses and maps to a cookie.
#[test]
fn repo_session_toml_parses() {
    let loader = ConfigLoader::load_from_dir(workspace_config_dir())
        .expect("workspace config directory loads");
    let config = SessionConfig::from_loader(&loader).expect("repo session.toml parses");
    assert_eq!(config.driver, "memory");
    assert_eq!(config.lifetime_minutes, 120);
    assert_eq!(config.ttl_secs(), 7_200);
    assert_eq!(config.cookie_name, "rustasea-session");

    let cookie = config.to_cookie_config().expect("cookie maps");
    assert_eq!(cookie.name, "rustasea-session");
    assert_eq!(cookie.same_site, SameSite::Lax);
    assert!(cookie.http_only);
}

/// The real repository `config/auth.toml` parses with the default guard wired.
#[test]
fn repo_auth_toml_parses() {
    use rustasea_auth::AuthConfig;

    let loader = ConfigLoader::load_from_dir(workspace_config_dir())
        .expect("workspace config directory loads");
    let auth = AuthConfig::from_loader(&loader).expect("repo auth.toml parses");
    assert_eq!(auth.default_guard(), "web");
    let provider = auth
        .provider_for("web")
        .expect("web guard provider resolves");
    assert_eq!(provider.driver, "eloquent");
    assert_eq!(auth.password_timeout, 10_800);
}
