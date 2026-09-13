//! End-to-end test for config-driven Fortify feature wiring (CFG-011).
//!
//! Exercises the public API only: the shipped repository `config/fortify.toml`
//! parses into the typed [`FortifyConfig`], and [`FortifyConfig::resolve_passkeys`]
//! derives the WebAuthn relying party from the shipped `config/app.toml` through
//! [`rustasea_foundation::AppConfig`].
use std::path::PathBuf;

use rustasea_auth::{FortifyConfig, ResolvedPasskeys};
use rustasea_config::ConfigLoader;
use rustasea_foundation::AppConfig;

/// Path to the workspace-root `config/` directory.
fn workspace_config_dir() -> PathBuf {
    // `CARGO_MANIFEST_DIR` = `<workspace>/crates/rustasea-auth`; the workspace
    // root is two levels up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("config")
}

/// The real repository `config/fortify.toml` parses into the full shape.
#[test]
fn repo_fortify_toml_parses() {
    let loader = ConfigLoader::load_from_dir(workspace_config_dir())
        .expect("workspace config directory loads");
    let config = FortifyConfig::from_loader(&loader).expect("repo fortify.toml parses");

    assert_eq!(config.guard, "web");
    assert_eq!(config.passwords, "users");
    assert_eq!(config.username, "email");
    assert_eq!(config.email, "email");
    assert!(config.lowercase_usernames);
    assert_eq!(config.home, "/dashboard");
    assert_eq!(config.prefix, "");
    assert_eq!(config.middleware, vec!["web".to_string()]);
    assert!(config.views);
    assert_eq!(config.limiters.login, "login");
    assert_eq!(config.passkeys.timeout, 60_000);
    assert!(config.features.registration);
    assert!(config.features.reset_passwords);
    assert!(config.features.email_verification);
    assert!(config.features.two_factor_authentication.confirm);
    assert!(config.features.passkeys.confirm_password);
}

/// The shipped `fortify.toml` resolves passkeys against the shipped `app.toml`.
#[test]
fn repo_fortify_resolves_passkeys_from_app_config() {
    let loader = ConfigLoader::load_from_dir(workspace_config_dir())
        .expect("workspace config directory loads");
    let config = FortifyConfig::from_loader(&loader).expect("repo fortify.toml parses");
    let app = AppConfig::from_loader(&loader).expect("repo app.toml parses");

    let resolved: ResolvedPasskeys = config.resolve_passkeys(&app);
    // Shipped `app.url` is `http://localhost:8000`, so the host is `localhost`.
    assert_eq!(resolved.relying_party_id, "localhost");
    assert_eq!(resolved.allowed_origins, vec![app.url.clone()]);
    assert_eq!(resolved.timeout, 60_000);
    // Shipped `app_key` is blank (normalised to `None`), so the secret is empty
    // until `PASSKEYS_USER_HANDLE_SECRET` or `APP_KEY` is supplied.
    assert_eq!(resolved.user_handle_secret, "");
}
