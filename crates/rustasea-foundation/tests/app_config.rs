//! Integration tests for the typed application configuration.
//!
//! Positive cases parse the *shipped* `config/app.toml` through
//! [`rustasea_config::ConfigLoader`] and assert Laravel `app.php` parity;
//! environment-override cases prove the loader's env layer wins over the file;
//! negative cases prove a malformed boolean surfaces as a typed
//! [`AppConfigError`] rather than being silently swallowed.

use std::path::Path;
use std::sync::Mutex;

use rustasea_config::ConfigLoader;
use rustasea_foundation::{AppConfig, AppConfigError};
use tempfile::TempDir;

/// Serializes tests that mutate process-global environment variables.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Environment variables this crate reads; cleared before file-only tests so a
/// developer's shell cannot leak into the assertions.
const APP_ENV_KEYS: &[&str] = &[
    "APP_NAME",
    "APP_ENV",
    "APP_DEBUG",
    "APP_URL",
    "APP_KEY",
    "APP_LOCALE",
    "APP_FALLBACK_LOCALE",
    "APP_MAINTENANCE_DRIVER",
    "APP_MAINTENANCE_STORE",
];

/// Create a fresh temporary directory (removed when dropped).
fn tempdir() -> TempDir {
    tempfile::tempdir().expect("create temp dir")
}

/// Remove every `APP_*` variable this crate reads.
fn clear_app_env() {
    for key in APP_ENV_KEYS {
        std::env::remove_var(key);
    }
}

/// Load the repository's shipped `config/` directory.
fn shipped_loader() -> ConfigLoader {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
    ConfigLoader::load_from_dir(dir).expect("load shipped config")
}

#[test]
fn shipped_app_toml_parses_with_full_app_php_parity() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_app_env();

    let config = AppConfig::from_loader(&shipped_loader()).expect("parse shipped app.toml");

    assert_eq!(config.name, "RustaSea");
    assert_eq!(config.env, "local");
    assert!(config.debug);
    assert_eq!(config.url, "http://localhost:8000");
    assert_eq!(config.timezone, "UTC");
    assert_eq!(config.locale, "en");
    assert_eq!(config.fallback_locale, "en");
    assert_eq!(config.faker_locale, "en_US");
    assert_eq!(config.cipher, "AES-256-CBC");
    assert_eq!(config.key, None, "blank app_key normalises to None");
    assert!(config.previous_keys.is_empty());
    assert_eq!(config.maintenance.driver, "file");
    assert_eq!(config.maintenance.store, "database");
}

#[test]
fn environment_overrides_win_over_the_file() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_app_env();

    std::env::set_var("APP_NAME", "Overridden");
    std::env::set_var("APP_ENV", "production");
    std::env::set_var("APP_DEBUG", "false");
    std::env::set_var("APP_URL", "https://example.test");
    std::env::set_var("APP_KEY", "base64:override");
    std::env::set_var("APP_LOCALE", "fr");
    std::env::set_var("APP_FALLBACK_LOCALE", "de");
    std::env::set_var("APP_MAINTENANCE_DRIVER", "database");

    let config = AppConfig::from_loader(&shipped_loader()).expect("parse with env overrides");
    clear_app_env();

    assert_eq!(config.name, "Overridden");
    assert_eq!(config.env, "production");
    assert!(
        !config.debug,
        "APP_DEBUG=false must override app_debug = true"
    );
    assert_eq!(config.url, "https://example.test");
    assert_eq!(config.key.as_deref(), Some("base64:override"));
    assert_eq!(config.locale, "fr");
    assert_eq!(config.fallback_locale, "de");
    assert_eq!(config.maintenance.driver, "database");
}

#[test]
fn app_debug_accepts_truthy_and_falsy_spellings() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_app_env();

    for truthy in ["1", "true", "on", "yes", "TRUE"] {
        std::env::set_var("APP_DEBUG", truthy);
        let config = AppConfig::from_loader(&shipped_loader()).expect("truthy APP_DEBUG");
        assert!(config.debug, "APP_DEBUG={truthy} should be true");
    }
    for falsy in ["0", "false", "off", "no"] {
        std::env::set_var("APP_DEBUG", falsy);
        let config = AppConfig::from_loader(&shipped_loader()).expect("falsy APP_DEBUG");
        assert!(!config.debug, "APP_DEBUG={falsy} should be false");
    }
    clear_app_env();
}

#[test]
fn blank_environment_key_is_treated_as_unset() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_app_env();

    std::env::set_var("APP_KEY", "   ");
    let config = AppConfig::from_loader(&shipped_loader()).expect("blank APP_KEY");
    clear_app_env();

    assert_eq!(config.key, None);
}

#[test]
fn absent_keys_fall_back_to_laravel_defaults() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_app_env();

    let dir = tempdir();
    let loader = ConfigLoader::load_from_dir(dir.path()).expect("load empty dir");
    let config = AppConfig::from_loader(&loader).expect("parse empty config");

    assert_eq!(config.name, "RustaSea");
    assert_eq!(config.env, "production");
    assert!(!config.debug);
    assert_eq!(config.timezone, "UTC");
    assert_eq!(config.locale, "en");
    assert_eq!(config.faker_locale, "en_US");
    assert_eq!(config.cipher, "AES-256-CBC");
    assert_eq!(config.key, None);
    assert_eq!(config.maintenance.driver, "file");
    assert_eq!(config.maintenance.store, "database");
}

#[test]
fn previous_keys_parse_as_an_array() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_app_env();

    let dir = tempdir();
    std::fs::write(
        dir.path().join("app.toml"),
        "app_key = \"base64:new\"\napp_previous_keys = [\"base64:old-1\", \"base64:old-2\"]\n",
    )
    .expect("write temp app.toml");

    let loader = ConfigLoader::load_from_dir(dir.path()).expect("load temp config");
    let config = AppConfig::from_loader(&loader).expect("parse temp config");

    assert_eq!(config.key.as_deref(), Some("base64:new"));
    assert_eq!(config.previous_keys, vec!["base64:old-1", "base64:old-2"]);
}

#[test]
fn malformed_bool_in_file_is_a_typed_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_app_env();

    let dir = tempdir();
    std::fs::write(dir.path().join("app.toml"), "app_debug = \"notabool\"\n")
        .expect("write malformed app.toml");

    let loader = ConfigLoader::load_from_dir(dir.path()).expect("load temp config");
    let error = AppConfig::from_loader(&loader).expect_err("malformed bool must fail");

    assert!(
        matches!(error, AppConfigError::Invalid(_)),
        "expected AppConfigError::Invalid, got {error:?}"
    );
}

#[test]
fn malformed_bool_from_environment_is_a_typed_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_app_env();

    std::env::set_var("APP_DEBUG", "notabool");
    let result = AppConfig::from_loader(&shipped_loader());
    clear_app_env();

    let error = result.expect_err("malformed APP_DEBUG must fail");
    assert!(
        matches!(error, AppConfigError::Invalid(_)),
        "expected AppConfigError::Invalid, got {error:?}"
    );
}

#[test]
fn malformed_array_type_is_a_typed_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_app_env();

    let dir = tempdir();
    std::fs::write(dir.path().join("app.toml"), "app_previous_keys = 42\n")
        .expect("write malformed app.toml");

    let loader = ConfigLoader::load_from_dir(dir.path()).expect("load temp config");
    let error = AppConfig::from_loader(&loader).expect_err("scalar for array must fail");

    assert!(
        matches!(error, AppConfigError::Invalid(_)),
        "expected AppConfigError::Invalid, got {error:?}"
    );
}

#[test]
fn database_driver_never_reports_maintenance() {
    let mut config = AppConfig::default();
    config.maintenance.driver = "database".to_string();
    assert!(
        !config.is_maintenance(),
        "the database driver is not probed and must report false"
    );
}

#[test]
fn file_driver_reports_maintenance_only_when_the_marker_exists() {
    // The marker is resolved relative to the process working directory, which
    // `cargo test` sets to the crate root. Guard with the env lock because the
    // file is process-global, and remove it afterwards.
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let config = AppConfig::default();
    assert_eq!(config.maintenance.driver, "file");

    let marker = Path::new("storage/framework/down");
    let preexisting = marker.exists();
    if preexisting {
        std::fs::remove_file(marker).expect("clear pre-existing marker");
    }
    assert!(!config.is_maintenance(), "no marker -> not in maintenance");

    let framework_dir = marker.parent().expect("marker parent");
    let created_framework = !framework_dir.exists();
    std::fs::create_dir_all(framework_dir).expect("create marker dir");
    std::fs::write(marker, "").expect("write marker");
    let in_maintenance = config.is_maintenance();
    std::fs::remove_file(marker).expect("remove marker");
    if created_framework {
        let _ = std::fs::remove_dir(framework_dir);
        let _ = std::fs::remove_dir("storage");
    }

    assert!(in_maintenance, "marker present -> in maintenance");
}
