//! Unit tests for typed `[session]` parsing, cookie/policy mapping, driver
//! validation, and the `SESSION_*` environment bridge.
//!
//! Uses throw-away temp directories fed to
//! [`rustasea_config::ConfigLoader::load_from_dir`] so the real `config/`
//! directory is never touched and the tests are hermetic. Environment-mutating
//! cases hold the crate-wide [`ENV_LOCK`] so the process-global `SESSION_*`
//! variables stay deterministic under parallel execution.
use super::*;
use crate::config::ENV_LOCK;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Every `SESSION_*` variable the [`SessionConfig::apply_env`] bridge reads.
const SESSION_ENV_KEYS: &[&str] = &[
    "SESSION_DRIVER",
    "SESSION_LIFETIME",
    "SESSION_COOKIE",
    "SESSION_SECURE",
    "SESSION_SECURE_COOKIE",
    "SESSION_SAME_SITE",
    "SESSION_EXPIRE_ON_CLOSE",
    "SESSION_ENCRYPT",
    "SESSION_PARTITIONED_COOKIE",
    "SESSION_HTTP_ONLY",
    "SESSION_CONNECTION",
    "SESSION_TABLE",
    "SESSION_STORE",
    "SESSION_PATH",
    "SESSION_DOMAIN",
];

/// Remove every `SESSION_*` override variable this module reads.
fn clear_session_env() {
    for key in SESSION_ENV_KEYS {
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
            "rustasea-auth-session-{}-{}",
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

/// Canonical `session.toml` body used across the positive tests.
const SESSION_TOML: &str = r#"
[session]
driver = "memory"
lifetime = 120
expire_on_close = false
encrypt = false
files = "storage/framework/sessions"
connection = "default"
table = "sessions"
store = "default"
lottery = [2, 100]
cookie = "rustasea-session"
path = "/"
domain = ""
secure = false
http_only = true
same_site = "lax"
partitioned = false
serialization = "json"
"#;

/// Load a loader over a temp dir containing only `session.toml`.
fn loader_with(session: &str) -> (TempConfigDir, ConfigLoader) {
    let dir = TempConfigDir::new();
    dir.write("session.toml", session);
    let loader = ConfigLoader::load_from_dir(dir.path()).expect("load temp config");
    (dir, loader)
}

/// A complete `session.toml` parses into the full Laravel shape.
#[test]
fn parses_full_session_shape() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_session_env();
    let (_dir, loader) = loader_with(SESSION_TOML);

    let session = SessionConfig::from_loader(&loader).expect("session parses");
    assert_eq!(session.driver, "memory");
    assert_eq!(session.lifetime_minutes, 120);
    assert!(!session.expire_on_close);
    assert!(!session.encrypt);
    assert_eq!(session.connection, "default");
    assert_eq!(session.table, "sessions");
    assert_eq!(session.store, "default");
    assert_eq!(session.lottery, [2, 100]);
    assert_eq!(session.cookie_name, "rustasea-session");
    assert_eq!(session.path, "/");
    assert_eq!(session.domain, Some(String::new()));
    assert!(!session.secure);
    assert!(session.http_only);
    assert_eq!(session.same_site, "lax");
    assert!(!session.partitioned);
    assert_eq!(session.serialization, "json");
}

/// A missing `[session]` table yields the documented defaults.
#[test]
fn missing_table_yields_defaults() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_session_env();
    let (_dir, loader) = loader_with("# empty\n");
    let session = SessionConfig::from_loader(&loader).expect("session defaults");
    assert_eq!(session.driver, "memory");
    assert_eq!(session.lifetime_minutes, 120);
    assert_eq!(session.cookie_name, "rustasea-session");
}

/// The session cookie is built with every configured flag.
#[test]
fn builds_cookie_from_config() {
    let session = SessionConfig {
        secure: true,
        http_only: true,
        same_site: "strict".to_string(),
        path: "/app".to_string(),
        domain: Some("example.com".to_string()),
        ..SessionConfig::default()
    };
    let cookie = session.to_cookie_config().expect("cookie config");
    assert_eq!(cookie.name, "rustasea-session");
    assert!(cookie.secure);
    assert!(cookie.http_only);
    assert_eq!(cookie.same_site, SameSite::Strict);
    assert_eq!(cookie.path, "/app");
    assert_eq!(cookie.domain.as_deref(), Some("example.com"));

    let built = cookie.build_cookie("sid");
    assert_eq!(built.name(), "rustasea-session");
    assert_eq!(built.secure(), Some(true));
    assert_eq!(built.http_only(), Some(true));
    assert_eq!(built.same_site(), Some(SameSite::Strict));
    assert_eq!(built.path(), Some("/app"));
    assert_eq!(built.domain(), Some("example.com"));
}

/// An empty domain string normalizes to host-only (`None`).
#[test]
fn empty_domain_is_host_only() {
    let session = SessionConfig {
        domain: Some(String::new()),
        ..SessionConfig::default()
    };
    assert_eq!(session.normalized_domain(), None);
    assert_eq!(
        session.to_cookie_config().expect("cookie").domain,
        None,
        "blank domain must not be emitted"
    );
}

/// The TTL is derived from the minute lifetime.
#[test]
fn ttl_derives_from_lifetime_minutes() {
    let session = SessionConfig {
        lifetime_minutes: 120,
        ..SessionConfig::default()
    };
    assert_eq!(session.ttl_secs(), 7_200);

    let session = SessionConfig {
        lifetime_minutes: 0,
        ..SessionConfig::default()
    };
    assert_eq!(session.ttl_secs(), 0);
}

/// `same_site` maps lax/strict/none case-insensitively.
#[test]
fn same_site_maps_all_variants() {
    for (raw, expected) in [
        ("lax", SameSite::Lax),
        ("Strict", SameSite::Strict),
        ("NONE", SameSite::None),
    ] {
        let session = SessionConfig {
            same_site: raw.to_string(),
            ..SessionConfig::default()
        };
        assert_eq!(session.same_site().expect("valid"), expected);
    }
}

/// The policy prefix is derived from the cookie name with a `-session-` marker.
#[test]
fn policy_prefix_carries_session_marker() {
    let session = SessionConfig::default();
    let policy = session.to_policy().expect("policy");
    assert_eq!(policy.prefix, "rustasea-session-");
    assert!(policy.validate_prefix().is_ok());
    assert_eq!(policy.user_key(), "rustasea-session-user");
}

/// An unrecognised `same_site` string is a typed error.
#[test]
fn invalid_same_site_is_typed_error() {
    let session = SessionConfig {
        same_site: "weird".to_string(),
        ..SessionConfig::default()
    };
    let err = session.same_site().expect_err("rejected");
    assert_eq!(err, AuthConfigError::InvalidSameSite("weird".to_string()));
    assert_eq!(err.code(), "AuthConfigError::InvalidSameSite");
    assert_eq!(
        session.to_cookie_config().expect_err("rejected"),
        AuthConfigError::InvalidSameSite("weird".to_string())
    );
}

/// A non-`json` serialization is a typed error.
#[test]
fn non_json_serialization_is_typed_error() {
    let session = SessionConfig {
        serialization: "msgpack".to_string(),
        ..SessionConfig::default()
    };
    let err = session.to_policy().expect_err("rejected");
    assert_eq!(
        err,
        AuthConfigError::UnsupportedSerialization("msgpack".to_string())
    );
}

/// A cookie name without the `-session-` marker is rejected by `to_policy`.
#[test]
fn cookie_name_without_marker_is_typed_error() {
    let session = SessionConfig {
        cookie_name: "rustasea_sess".to_string(),
        ..SessionConfig::default()
    };
    assert!(matches!(
        session.to_policy().expect_err("rejected"),
        AuthConfigError::Invalid(_)
    ));
}

/// An unimplemented session driver is rejected at load time.
#[test]
fn unimplemented_session_driver_is_typed_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_session_env();
    let session_toml = SESSION_TOML.replace("driver = \"memory\"", "driver = \"redis\"");
    let (_dir, loader) = loader_with(&session_toml);
    let err = SessionConfig::from_loader(&loader).expect_err("rejected");
    assert_eq!(
        err,
        AuthConfigError::UnsupportedSessionDriver("redis".to_string())
    );
    assert_eq!(err.code(), "AuthConfigError::UnsupportedSessionDriver");
}

/// Every documented `SESSION_*` variable overrides its file counterpart.
#[test]
fn session_env_overrides_win_over_the_file() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_session_env();
    std::env::set_var("SESSION_DRIVER", "memory");
    std::env::set_var("SESSION_LIFETIME", "45");
    std::env::set_var("SESSION_COOKIE", "tenant-session");
    std::env::set_var("SESSION_SECURE", "true");
    std::env::set_var("SESSION_SAME_SITE", "strict");
    std::env::set_var("SESSION_EXPIRE_ON_CLOSE", "true");
    std::env::set_var("SESSION_ENCRYPT", "true");
    std::env::set_var("SESSION_PARTITIONED_COOKIE", "true");
    std::env::set_var("SESSION_HTTP_ONLY", "false");
    std::env::set_var("SESSION_CONNECTION", "pgsql");
    std::env::set_var("SESSION_TABLE", "user_sessions");
    std::env::set_var("SESSION_STORE", "redis-store");
    std::env::set_var("SESSION_PATH", "/app");
    std::env::set_var("SESSION_DOMAIN", "example.com");

    let (_dir, loader) = loader_with(SESSION_TOML);
    let session = SessionConfig::from_loader(&loader).expect("session parses");
    clear_session_env();

    assert_eq!(session.driver, "memory");
    assert_eq!(session.lifetime_minutes, 45);
    assert_eq!(session.cookie_name, "tenant-session");
    assert!(session.secure);
    assert_eq!(session.same_site, "strict");
    assert!(session.expire_on_close);
    assert!(session.encrypt);
    assert!(session.partitioned);
    assert!(!session.http_only);
    assert_eq!(session.connection, "pgsql");
    assert_eq!(session.table, "user_sessions");
    assert_eq!(session.store, "redis-store");
    assert_eq!(session.path, "/app");
    assert_eq!(session.domain.as_deref(), Some("example.com"));
}

/// `SESSION_SECURE_COOKIE` is honoured as an alias of `SESSION_SECURE`.
#[test]
fn session_secure_cookie_alias_is_honoured() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_session_env();
    std::env::set_var("SESSION_SECURE_COOKIE", "true");

    let (_dir, loader) = loader_with(SESSION_TOML);
    let session = SessionConfig::from_loader(&loader).expect("session parses");
    clear_session_env();

    assert!(
        session.secure,
        "SESSION_SECURE_COOKIE must set the secure flag"
    );
}

/// When both are set, the explicit `SESSION_SECURE` wins over the alias.
#[test]
fn explicit_session_secure_wins_over_alias() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_session_env();
    std::env::set_var("SESSION_SECURE", "false");
    std::env::set_var("SESSION_SECURE_COOKIE", "true");

    let (_dir, loader) = loader_with(SESSION_TOML);
    let session = SessionConfig::from_loader(&loader).expect("session parses");
    clear_session_env();

    assert!(
        !session.secure,
        "explicit SESSION_SECURE must win over SESSION_SECURE_COOKIE"
    );
}

/// A blank `SESSION_*` value is ignored and the file value survives.
#[test]
fn session_blank_env_is_ignored() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_session_env();
    std::env::set_var("SESSION_LIFETIME", "   ");
    std::env::set_var("SESSION_COOKIE", "");
    std::env::set_var("SESSION_SAME_SITE", "  ");
    std::env::set_var("SESSION_DOMAIN", "  ");
    std::env::set_var("SESSION_CONNECTION", "");

    let (_dir, loader) = loader_with(SESSION_TOML);
    let session = SessionConfig::from_loader(&loader).expect("session parses");
    clear_session_env();

    assert_eq!(session.lifetime_minutes, 120);
    assert_eq!(session.cookie_name, "rustasea-session");
    assert_eq!(session.same_site, "lax");
    assert_eq!(session.domain, Some(String::new()));
    assert_eq!(session.connection, "default");
}

/// A non-integer `SESSION_LIFETIME` is a typed invalid-config error.
#[test]
fn session_lifetime_non_integer_is_typed_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_session_env();
    std::env::set_var("SESSION_LIFETIME", "soon");

    let (_dir, loader) = loader_with(SESSION_TOML);
    let err = SessionConfig::from_loader(&loader).expect_err("rejected");
    clear_session_env();

    assert!(matches!(err, AuthConfigError::Invalid(_)), "got {err}");
    assert_eq!(err.code(), "AuthConfigError::Invalid");
}

/// A non-boolean `SESSION_SECURE` is a typed invalid-config error.
#[test]
fn session_secure_non_bool_is_typed_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_session_env();
    std::env::set_var("SESSION_SECURE", "maybe");

    let (_dir, loader) = loader_with(SESSION_TOML);
    let err = SessionConfig::from_loader(&loader).expect_err("rejected");
    clear_session_env();

    assert!(matches!(err, AuthConfigError::Invalid(_)), "got {err}");
    assert_eq!(err.code(), "AuthConfigError::Invalid");
}

/// A non-boolean alias (`SESSION_SECURE_COOKIE`) is a typed invalid-config error.
#[test]
fn session_secure_cookie_alias_non_bool_is_typed_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_session_env();
    std::env::set_var("SESSION_SECURE_COOKIE", "maybe");

    let (_dir, loader) = loader_with(SESSION_TOML);
    let err = SessionConfig::from_loader(&loader).expect_err("rejected");
    clear_session_env();

    assert!(matches!(err, AuthConfigError::Invalid(_)), "got {err}");
    assert_eq!(err.code(), "AuthConfigError::Invalid");
}

/// A non-boolean `SESSION_HTTP_ONLY` is a typed invalid-config error.
#[test]
fn session_http_only_non_bool_is_typed_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_session_env();
    std::env::set_var("SESSION_HTTP_ONLY", "maybe");

    let (_dir, loader) = loader_with(SESSION_TOML);
    let err = SessionConfig::from_loader(&loader).expect_err("rejected");
    clear_session_env();

    assert!(matches!(err, AuthConfigError::Invalid(_)), "got {err}");
    assert_eq!(err.code(), "AuthConfigError::Invalid");
}
