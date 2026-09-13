//! Unit tests for typed `fortify.toml` parsing, env overrides, and passkey
//! resolution.
//!
//! Uses throw-away temp directories fed to
//! [`rustasea_config::ConfigLoader::load_from_dir`] so the real `config/`
//! directory is never touched and the tests are hermetic. Environment-mutating
//! cases hold [`ENV_LOCK`] to stay deterministic under parallel execution.
use super::*;
use crate::config::ENV_LOCK;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Every environment variable this module's overrides read.
const FORTIFY_ENV_KEYS: &[&str] = &[
    "FORTIFY_GUARD",
    "FORTIFY_PASSWORDS",
    "FORTIFY_USERNAME",
    "FORTIFY_EMAIL",
    "FORTIFY_LOWERCASE_USERNAMES",
    "FORTIFY_HOME",
    "FORTIFY_PREFIX",
    "FORTIFY_DOMAIN",
    "FORTIFY_MIDDLEWARE",
    "FORTIFY_VIEWS",
    "FORTIFY_LIMITERS_LOGIN",
    "FORTIFY_REGISTRATION",
    "FORTIFY_RESET_PASSWORDS",
    "FORTIFY_EMAIL_VERIFICATION",
    "PASSKEYS_USER_HANDLE_SECRET",
];

/// Unique temporary directory removed when dropped.
struct TempConfigDir {
    path: PathBuf,
}

impl TempConfigDir {
    /// Create an empty temp directory with a process-unique name.
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "rustasea-auth-fortify-{}-{}",
            std::process::id(),
            unique
        ));
        fs::create_dir_all(&path).expect("create temp config dir");
        Self { path }
    }

    /// Write `contents` to `name` inside the temp directory.
    fn write(&self, name: &str, contents: &str) {
        fs::write(self.path.join(name), contents).expect("write temp config file");
    }

    /// Borrow the temp directory path.
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempConfigDir {
    /// Remove the temp directory and all of its contents.
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Canonical `fortify.toml` body used across the positive tests.
const FORTIFY_TOML: &str = r#"
[fortify]
guard = "web"
passwords = "users"
username = "email"
email = "email"
lowercase_usernames = true
home = "/dashboard"
prefix = ""
middleware = ["web"]
views = true

[fortify.limiters]
login = "login"

[fortify.passkeys]
user_handle_secret = ""
timeout = 60000

[fortify.features]
registration = true
reset_passwords = true
email_verification = true

[fortify.features.two_factor_authentication]
confirm = true
confirm_password = true

[fortify.features.passkeys]
confirm_password = true
"#;

/// Remove every override variable this module reads.
fn clear_fortify_env() {
    for key in FORTIFY_ENV_KEYS {
        std::env::remove_var(key);
    }
}

/// Load a loader over a temp dir containing `fortify.toml`.
fn loader_with(fortify: &str) -> (TempConfigDir, ConfigLoader) {
    let dir = TempConfigDir::new();
    dir.write("fortify.toml", fortify);
    let loader = ConfigLoader::load_from_dir(dir.path()).expect("load temp config");
    (dir, loader)
}

/// A complete `fortify.toml` parses into the full Laravel Fortify shape.
#[test]
fn parses_full_fortify_shape() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_fortify_env();
    let (_dir, loader) = loader_with(FORTIFY_TOML);

    let config = FortifyConfig::from_loader(&loader).expect("fortify parses");
    assert_eq!(config.guard, "web");
    assert_eq!(config.passwords, "users");
    assert_eq!(config.username, "email");
    assert_eq!(config.email, "email");
    assert!(config.lowercase_usernames);
    assert_eq!(config.home, "/dashboard");
    assert_eq!(config.prefix, "");
    assert_eq!(config.domain, None);
    assert_eq!(config.middleware, vec!["web".to_string()]);
    assert!(config.views);
    assert_eq!(config.limiters.login, "login");
    assert_eq!(config.limiters.two_factor, None);
    assert_eq!(config.limiters.passkeys, None);
    assert_eq!(config.passkeys.user_handle_secret, "");
    assert_eq!(config.passkeys.timeout, 60_000);
    assert!(config.features.registration);
    assert!(config.features.reset_passwords);
    assert!(config.features.email_verification);
    assert!(config.features.two_factor_authentication.confirm);
    assert!(config.features.two_factor_authentication.confirm_password);
    assert!(config.features.passkeys.confirm_password);
}

/// A missing `[fortify]` table yields the documented defaults.
#[test]
fn missing_table_yields_defaults() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_fortify_env();
    let (_dir, loader) = loader_with("# empty\n");

    let config = FortifyConfig::from_loader(&loader).expect("defaults tolerated");
    assert_eq!(config, FortifyConfig::default());
    assert_eq!(config.guard, "web");
    assert_eq!(config.home, "/dashboard");
    assert!(config.views);
    assert!(config.features.registration);
}

/// A malformed boolean in the file is a typed invalid-config error.
#[test]
fn invalid_bool_in_file_is_typed_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_fortify_env();
    let toml = FORTIFY_TOML.replace("views = true", "views = \"nope\"");
    let (_dir, loader) = loader_with(&toml);

    let err = FortifyConfig::from_loader(&loader).expect_err("rejected");
    assert!(matches!(err, AuthConfigError::Invalid(_)), "got {err}");
    assert_eq!(err.code(), "AuthConfigError::Invalid");
}

/// A malformed integer in the file is a typed invalid-config error.
#[test]
fn invalid_int_in_file_is_typed_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_fortify_env();
    let toml = FORTIFY_TOML.replace("timeout = 60000", "timeout = \"soon\"");
    let (_dir, loader) = loader_with(&toml);

    let err = FortifyConfig::from_loader(&loader).expect_err("rejected");
    assert!(matches!(err, AuthConfigError::Invalid(_)), "got {err}");
}

/// Every documented `FORTIFY_*` variable overrides its file counterpart.
#[test]
fn env_overrides_win_over_the_file() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_fortify_env();
    std::env::set_var("FORTIFY_GUARD", "api");
    std::env::set_var("FORTIFY_PASSWORDS", "admins");
    std::env::set_var("FORTIFY_USERNAME", "username");
    std::env::set_var("FORTIFY_EMAIL", "email_address");
    std::env::set_var("FORTIFY_LOWERCASE_USERNAMES", "false");
    std::env::set_var("FORTIFY_HOME", "/home");
    std::env::set_var("FORTIFY_PREFIX", "fortify");
    std::env::set_var("FORTIFY_DOMAIN", "auth.example.com");
    std::env::set_var("FORTIFY_MIDDLEWARE", "web, api ,  auth");
    std::env::set_var("FORTIFY_VIEWS", "false");
    std::env::set_var("FORTIFY_LIMITERS_LOGIN", "custom-login");
    std::env::set_var("FORTIFY_REGISTRATION", "false");
    std::env::set_var("FORTIFY_RESET_PASSWORDS", "false");
    std::env::set_var("FORTIFY_EMAIL_VERIFICATION", "false");
    std::env::set_var("PASSKEYS_USER_HANDLE_SECRET", "env-secret");

    let (_dir, loader) = loader_with(FORTIFY_TOML);
    let config = FortifyConfig::from_loader(&loader).expect("fortify parses");
    clear_fortify_env();

    assert_eq!(config.guard, "api");
    assert_eq!(config.passwords, "admins");
    assert_eq!(config.username, "username");
    assert_eq!(config.email, "email_address");
    assert!(!config.lowercase_usernames);
    assert_eq!(config.home, "/home");
    assert_eq!(config.prefix, "fortify");
    assert_eq!(config.domain.as_deref(), Some("auth.example.com"));
    assert_eq!(
        config.middleware,
        vec!["web".to_string(), "api".to_string(), "auth".to_string()]
    );
    assert!(!config.views);
    assert_eq!(config.limiters.login, "custom-login");
    assert!(!config.features.registration);
    assert!(!config.features.reset_passwords);
    assert!(!config.features.email_verification);
    assert_eq!(config.passkeys.user_handle_secret, "env-secret");
}

/// A blank `FORTIFY_*` value is ignored and the file value survives.
#[test]
fn blank_env_is_ignored() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_fortify_env();
    std::env::set_var("FORTIFY_GUARD", "   ");
    std::env::set_var("FORTIFY_HOME", "");
    std::env::set_var("PASSKEYS_USER_HANDLE_SECRET", "  ");

    let (_dir, loader) = loader_with(FORTIFY_TOML);
    let config = FortifyConfig::from_loader(&loader).expect("fortify parses");
    clear_fortify_env();

    assert_eq!(config.guard, "web");
    assert_eq!(config.home, "/dashboard");
    assert_eq!(config.passkeys.user_handle_secret, "");
}

/// A non-boolean `FORTIFY_*` override is a typed invalid-config error.
#[test]
fn env_non_bool_is_typed_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_fortify_env();
    std::env::set_var("FORTIFY_VIEWS", "maybe");

    let (_dir, loader) = loader_with(FORTIFY_TOML);
    let err = FortifyConfig::from_loader(&loader).expect_err("rejected");
    clear_fortify_env();

    assert!(matches!(err, AuthConfigError::Invalid(_)), "got {err}");
    assert_eq!(err.code(), "AuthConfigError::Invalid");
}

/// `resolve_passkeys` derives the relying party from `app.url` when unset.
#[test]
fn resolve_passkeys_derives_from_app_url() {
    let config = FortifyConfig::default();
    let app = AppConfig {
        url: "https://app.example.com:8443/dashboard".to_string(),
        key: Some("base64:app-key".to_string()),
        ..AppConfig::default()
    };

    let resolved = config.resolve_passkeys(&app);
    assert_eq!(resolved.relying_party_id, "app.example.com");
    assert_eq!(
        resolved.allowed_origins,
        vec!["https://app.example.com:8443/dashboard".to_string()]
    );
    assert_eq!(resolved.user_handle_secret, "base64:app-key");
    assert_eq!(resolved.timeout, 60_000);
}

/// `resolve_passkeys` prefers explicit file values over the derived ones.
#[test]
fn resolve_passkeys_prefers_explicit_values() {
    let config = FortifyConfig {
        passkeys: FortifyPasskeysConfig {
            relying_party_id: Some("localhost".to_string()),
            allowed_origins: Some(vec!["http://localhost:8000".to_string()]),
            user_handle_secret: "explicit".to_string(),
            timeout: 30_000,
        },
        ..FortifyConfig::default()
    };
    let app = AppConfig {
        url: "https://ignored.example.com".to_string(),
        key: Some("base64:app-key".to_string()),
        ..AppConfig::default()
    };

    let resolved = config.resolve_passkeys(&app);
    assert_eq!(resolved.relying_party_id, "localhost");
    assert_eq!(
        resolved.allowed_origins,
        vec!["http://localhost:8000".to_string()]
    );
    assert_eq!(resolved.user_handle_secret, "explicit");
    assert_eq!(resolved.timeout, 30_000);
}

/// A blank user-handle secret falls back to `app.key`; an absent key yields "".
#[test]
fn resolve_passkeys_secret_falls_back_to_app_key() {
    let config = FortifyConfig::default();
    let app = AppConfig {
        key: None,
        ..AppConfig::default()
    };
    let resolved = config.resolve_passkeys(&app);
    assert_eq!(resolved.user_handle_secret, "");
    // Default `AppConfig::url` is `http://localhost`, so the host is `localhost`.
    assert_eq!(resolved.relying_party_id, "localhost");
    assert_eq!(
        resolved.allowed_origins,
        vec!["http://localhost".to_string()]
    );
}

/// The host extractor handles scheme, port, userinfo, path, and IPv6 literals.
#[test]
fn host_extraction_handles_url_shapes() {
    assert_eq!(
        host_from_url("http://localhost"),
        Some("localhost".to_string())
    );
    assert_eq!(
        host_from_url("http://localhost:8000/path?q=1#frag"),
        Some("localhost".to_string())
    );
    assert_eq!(
        host_from_url("https://user:pass@host.example.com:443"),
        Some("host.example.com".to_string())
    );
    assert_eq!(host_from_url("http://[::1]:8000"), Some("::1".to_string()));
    assert_eq!(
        host_from_url("localhost:8000"),
        Some("localhost".to_string())
    );
    assert_eq!(host_from_url(""), None);
}
