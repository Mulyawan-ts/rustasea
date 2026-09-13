//! Unit tests for typed `auth.toml` parsing, guard wiring, and the `AUTH_*`
//! environment bridge.
//!
//! Session-specific coverage (cookie mapping, driver validation, and the
//! `SESSION_*` environment bridge) lives in `config/session/tests.rs`. Uses
//! throw-away temp directories fed to
//! [`rustasea_config::ConfigLoader::load_from_dir`] so the real `config/`
//! directory is never touched and the tests are hermetic. Environment-mutating
//! cases hold the crate-wide [`ENV_LOCK`] so the process-global `AUTH_*` /
//! `SESSION_*` variables stay deterministic under parallel execution.
use super::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Every `AUTH_*` variable the [`AuthConfig::apply_env`] bridge reads.
const AUTH_ENV_KEYS: &[&str] = &[
    "AUTH_GUARD",
    "AUTH_PASSWORD_BROKER",
    "AUTH_MODEL",
    "AUTH_PASSWORD_RESET_TOKEN_TABLE",
    "AUTH_PASSWORD_TIMEOUT",
];

/// Remove every `AUTH_*` override variable this module reads.
fn clear_auth_env() {
    for key in AUTH_ENV_KEYS {
        std::env::remove_var(key);
    }
}

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
            "rustasea-auth-config-{}-{}",
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

/// Canonical `auth.toml` body used across the positive tests.
const AUTH_TOML: &str = r#"
[auth]
password_timeout = 10800

[auth.defaults]
guard = "web"
passwords = "users"

[auth.guards.web]
driver = "session"
provider = "users"

[auth.providers.users]
driver = "eloquent"
model = "App\\Models\\User"

[auth.passwords.users]
provider = "users"
table = "password_reset_tokens"
expire = 60
throttle = 60
"#;

/// Minimal `session.toml` body so the loader always has both tables.
const SESSION_TOML: &str = r#"
[session]
driver = "memory"
lifetime = 120
cookie = "rustasea-session"
same_site = "lax"
serialization = "json"
"#;

/// Load a loader over a temp dir containing `auth.toml` and `session.toml`.
fn loader_with(auth: &str, session: &str) -> (TempConfigDir, ConfigLoader) {
    let dir = TempConfigDir::new();
    dir.write("auth.toml", auth);
    dir.write("session.toml", session);
    let loader = ConfigLoader::load_from_dir(dir.path()).expect("load temp config");
    (dir, loader)
}

/// A complete auth.toml + session.toml parse into the full Laravel shape.
#[test]
fn parses_both_tomls_into_full_shape() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_auth_env();
    let (_dir, loader) = loader_with(AUTH_TOML, SESSION_TOML);

    let auth = AuthConfig::from_loader(&loader).expect("auth parses");
    assert_eq!(auth.default_guard(), "web");
    assert_eq!(auth.default_password_broker(), "users");
    assert_eq!(auth.password_timeout, 10_800);
    assert_eq!(auth.guards.len(), 1);
    assert_eq!(auth.guards["web"].driver, "session");
    assert_eq!(auth.providers["users"].driver, "eloquent");
    assert_eq!(
        auth.providers["users"].model.as_deref(),
        Some("App\\Models\\User")
    );
    assert_eq!(auth.passwords["users"].expire, 60);
    assert_eq!(auth.passwords["users"].throttle, 60);
}

/// A missing `[auth]` table yields the documented defaults.
#[test]
fn missing_table_yields_defaults() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_auth_env();
    let (_dir, loader) = loader_with("# empty\n", SESSION_TOML);
    let auth = AuthConfig::from_loader(&loader).expect("auth defaults");
    assert_eq!(auth.default_guard(), "web");
    assert_eq!(auth.password_timeout, 10_800);
    assert!(auth.guards.is_empty());
}

/// `provider_for` resolves the guard's declared provider.
#[test]
fn provider_for_resolves_declared_provider() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_auth_env();
    let (_dir, loader) = loader_with(AUTH_TOML, SESSION_TOML);
    let auth = AuthConfig::from_loader(&loader).expect("auth parses");
    let provider = auth.provider_for("web").expect("provider resolves");
    assert_eq!(provider.driver, "eloquent");
}

/// A guard naming an undeclared provider is a typed error.
#[test]
fn missing_guard_provider_is_typed_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_auth_env();
    let auth_toml = r#"
[auth.defaults]
guard = "web"

[auth.guards.web]
driver = "session"
provider = "users"
"#;
    let (_dir, loader) = loader_with(auth_toml, SESSION_TOML);
    let auth = AuthConfig::from_loader(&loader).expect("auth parses");
    let err = auth.provider_for("web").expect_err("rejected");
    assert_eq!(
        err,
        AuthConfigError::MissingGuardProvider {
            guard: "web".to_string(),
            provider: "users".to_string(),
        }
    );
    assert_eq!(err.code(), "AuthConfigError::MissingGuardProvider");
}

/// A guard with no `provider` key is a typed error.
#[test]
fn guard_without_provider_is_typed_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_auth_env();
    let auth_toml = r#"
[auth.guards.api]
driver = "token"
"#;
    let (_dir, loader) = loader_with(auth_toml, SESSION_TOML);
    let auth = AuthConfig::from_loader(&loader).expect("auth parses");
    let err = auth.provider_for("api").expect_err("rejected");
    assert_eq!(
        err,
        AuthConfigError::MissingGuardProvider {
            guard: "api".to_string(),
            provider: String::new(),
        }
    );
}

/// An unknown guard name is a typed error.
#[test]
fn unknown_guard_is_typed_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_auth_env();
    let (_dir, loader) = loader_with(AUTH_TOML, SESSION_TOML);
    let auth = AuthConfig::from_loader(&loader).expect("auth parses");
    assert_eq!(
        auth.provider_for("missing").expect_err("rejected"),
        AuthConfigError::UnknownGuard("missing".to_string())
    );
}

/// Every documented `AUTH_*` variable overrides its file counterpart.
#[test]
fn auth_env_overrides_win_over_the_file() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_auth_env();
    std::env::set_var("AUTH_GUARD", "api");
    std::env::set_var("AUTH_PASSWORD_BROKER", "admins");
    std::env::set_var("AUTH_MODEL", "App\\Models\\Admin");
    std::env::set_var("AUTH_PASSWORD_RESET_TOKEN_TABLE", "admin_password_resets");
    std::env::set_var("AUTH_PASSWORD_TIMEOUT", "3600");

    let (_dir, loader) = loader_with(AUTH_TOML, SESSION_TOML);
    let auth = AuthConfig::from_loader(&loader).expect("auth parses");
    clear_auth_env();

    assert_eq!(auth.default_guard(), "api");
    assert_eq!(auth.default_password_broker(), "admins");
    assert_eq!(
        auth.providers["users"].model.as_deref(),
        Some("App\\Models\\Admin")
    );
    assert_eq!(auth.passwords["users"].table, "admin_password_resets");
    assert_eq!(auth.password_timeout, 3_600);
}

/// A blank `AUTH_*` value is ignored and the file value survives.
#[test]
fn auth_blank_env_is_ignored() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_auth_env();
    std::env::set_var("AUTH_GUARD", "   ");
    std::env::set_var("AUTH_MODEL", "");
    std::env::set_var("AUTH_PASSWORD_TIMEOUT", "  ");

    let (_dir, loader) = loader_with(AUTH_TOML, SESSION_TOML);
    let auth = AuthConfig::from_loader(&loader).expect("auth parses");
    clear_auth_env();

    assert_eq!(auth.default_guard(), "web");
    assert_eq!(
        auth.providers["users"].model.as_deref(),
        Some("App\\Models\\User")
    );
    assert_eq!(auth.password_timeout, 10_800);
}

/// A non-integer `AUTH_PASSWORD_TIMEOUT` is a typed invalid-config error.
#[test]
fn auth_password_timeout_non_integer_is_typed_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_auth_env();
    std::env::set_var("AUTH_PASSWORD_TIMEOUT", "soon");

    let (_dir, loader) = loader_with(AUTH_TOML, SESSION_TOML);
    let err = AuthConfig::from_loader(&loader).expect_err("rejected");
    clear_auth_env();

    assert!(matches!(err, AuthConfigError::Invalid(_)), "got {err}");
    assert_eq!(err.code(), "AuthConfigError::Invalid");
}
