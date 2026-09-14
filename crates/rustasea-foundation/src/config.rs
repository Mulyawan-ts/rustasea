//! Typed application configuration — Laravel 13.x `config/app.php` parity.
//!
//! [`AppConfig`] mirrors every key of Laravel's `config/app.php`: application
//! identity (`name`, `env`, `debug`, `url`), localisation (`timezone`,
//! `locale`, `fallback_locale`, `faker_locale`), encryption (`cipher`, `key`,
//! `previous_keys`) and maintenance mode (`maintenance.driver`,
//! `maintenance.store`).
//!
//! # Flat keys and the environment overlay
//!
//! RustaSea keeps the loader's flat `app_*` key convention (see
//! `config/app.toml`) instead of a `[app]` table. The loader's environment
//! source maps process variables onto those same flat keys, so the overlay is
//! one-to-one and needs no bespoke mapping:
//!
//! | Environment variable       | Config key                |
//! | -------------------------- | ------------------------- |
//! | `APP_NAME`                 | `app_name`                |
//! | `APP_ENV`                  | `app_env`                 |
//! | `APP_DEBUG`                | `app_debug`               |
//! | `APP_URL`                  | `app_url`                 |
//! | `APP_KEY`                  | `app_key`                 |
//! | `APP_LOCALE`               | `app_locale`              |
//! | `APP_FALLBACK_LOCALE`      | `app_fallback_locale`     |
//! | `APP_MAINTENANCE_DRIVER`   | `app_maintenance_driver`  |
//! | `APP_MAINTENANCE_STORE`    | `app_maintenance_store`   |
//!
//! Because [`rustasea_config::ConfigLoader`] applies the environment as its
//! highest-precedence source, **environment variables always win over
//! `config/app.toml`**. `APP_DEBUG` accepts the usual truthy/falsy spellings
//! (`1`/`true`/`on`/`yes` and `0`/`false`/`off`/`no`); any other value is a
//! typed [`AppConfigError`].

use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;

use rustasea_config::ConfigLoader;

/// Default directory the boot-time [`ConfigLoader`] discovers (`config/*.toml`).
pub const DEFAULT_CONFIG_DIR: &str = "config";

/// Container key holding the boot-time [`ConfigLoader`] as an `Arc<ConfigLoader>`.
pub const CONFIG_LOADER_KEY: &str = "config.loader";

/// Load the boot-time config loader from `dir` (optional) + environment overlay.
///
/// Discovers every `config/*.toml` in `dir` and overlays the process environment
/// (env wins). A missing directory is tolerated — the loader then yields an
/// environment-only configuration. Malformed TOML surfaces the loader error as a
/// `String` the caller maps onto its boot error.
pub fn load_config_loader(dir: &Path) -> Result<Arc<ConfigLoader>, String> {
    ConfigLoader::load_from_dir(dir)
        .map(Arc::new)
        .map_err(|error| error.to_string())
}

/// Application name used when `app_name` is absent.
pub const DEFAULT_NAME: &str = "RustaSea";

/// Environment used when `app_env` is absent (Laravel's default).
pub const DEFAULT_ENV: &str = "production";

/// Timezone used when `app_timezone` is absent (Laravel's default).
pub const DEFAULT_TIMEZONE: &str = "UTC";

/// Locale used when `app_locale` is absent (Laravel's default).
pub const DEFAULT_LOCALE: &str = "en";

/// Fallback locale used when `app_fallback_locale` is absent.
pub const DEFAULT_FALLBACK_LOCALE: &str = "en";

/// Faker locale used when `app_faker_locale` is absent.
pub const DEFAULT_FAKER_LOCALE: &str = "en_US";

/// Encryption cipher used when `app_cipher` is absent.
pub const DEFAULT_CIPHER: &str = "AES-256-CBC";

/// Maintenance driver used when `app_maintenance_driver` is absent.
pub const DEFAULT_MAINTENANCE_DRIVER: &str = "file";

/// Maintenance store used when `app_maintenance_store` is absent.
pub const DEFAULT_MAINTENANCE_STORE: &str = "database";

/// Marker file checked by the `file` maintenance driver.
///
/// Mirrors Laravel, whose `file` driver considers maintenance active when
/// `storage/framework/down` exists relative to the project root.
pub const MAINTENANCE_MARKER: &str = "storage/framework/down";

/// Top-level error type for application configuration.
#[derive(Debug, thiserror::Error)]
pub enum AppConfigError {
    /// The `app_*` configuration exists but cannot be deserialized (bad TOML
    /// type, malformed boolean, …).
    #[error("app configuration invalid: {0}")]
    Invalid(String),
}

/// Maintenance-mode backend selection (`maintenance` in Laravel).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaintenanceConfig {
    /// Backing driver (`file` or `database`).
    pub driver: String,
    /// Store used by the `database` driver.
    pub store: String,
}

impl Default for MaintenanceConfig {
    /// Laravel defaults: the `file` driver backed by the `database` store.
    fn default() -> Self {
        Self {
            driver: DEFAULT_MAINTENANCE_DRIVER.to_string(),
            store: DEFAULT_MAINTENANCE_STORE.to_string(),
        }
    }
}

/// Typed application configuration.
///
/// Every field mirrors a key of Laravel's `config/app.php`. Build one with
/// [`AppConfig::from_loader`]; [`AppConfig::default`] returns the Laravel
/// defaults for an empty configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppConfig {
    /// Application display name.
    pub name: String,
    /// Runtime environment (`local`, `production`, …).
    pub env: String,
    /// Whether debug output is enabled.
    pub debug: bool,
    /// Canonical base URL of the application.
    pub url: String,
    /// Default timezone (Laravel ships `UTC`).
    pub timezone: String,
    /// Default locale.
    pub locale: String,
    /// Locale used when the default locale has no translation.
    pub fallback_locale: String,
    /// Faker locale used by database factories.
    pub faker_locale: String,
    /// Encryption cipher (Laravel ships `AES-256-CBC`).
    pub cipher: String,
    /// Encryption key; `None` when unset or blank.
    pub key: Option<String>,
    /// Previous keys retained for decryption during key rotation.
    pub previous_keys: Vec<String>,
    /// Maintenance-mode backend.
    pub maintenance: MaintenanceConfig,
}

impl Default for AppConfig {
    /// The Laravel defaults, with no encryption key configured.
    fn default() -> Self {
        Self {
            name: DEFAULT_NAME.to_string(),
            env: DEFAULT_ENV.to_string(),
            debug: false,
            url: "http://localhost".to_string(),
            timezone: DEFAULT_TIMEZONE.to_string(),
            locale: DEFAULT_LOCALE.to_string(),
            fallback_locale: DEFAULT_FALLBACK_LOCALE.to_string(),
            faker_locale: DEFAULT_FAKER_LOCALE.to_string(),
            cipher: DEFAULT_CIPHER.to_string(),
            key: None,
            previous_keys: Vec::new(),
            maintenance: MaintenanceConfig::default(),
        }
    }
}

impl AppConfig {
    /// Deserialize the application configuration from a layered loader.
    ///
    /// The loader merges `config/app.toml` (and every other `config/*.toml`)
    /// with the process environment, which wins. Unknown keys — including the
    /// other configuration tables — are ignored, and absent `app_*` keys fall
    /// back to the Laravel defaults.
    ///
    /// # Errors
    ///
    /// [`AppConfigError::Invalid`] when a present `app_*` key cannot be
    /// deserialized into its expected type (for example a non-boolean
    /// `APP_DEBUG`).
    pub fn from_loader(loader: &ConfigLoader) -> Result<Self, AppConfigError> {
        let file = loader
            .get::<AppConfigFile>()
            .map_err(|error| AppConfigError::Invalid(error.to_string()))?;
        Ok(Self::from(file))
    }

    /// True when the application is considered to be in maintenance mode.
    ///
    /// Pragmatic rule: RustaSea does not yet run Laravel's maintenance
    /// middleware, so this mirrors the `file` driver's rule — maintenance is
    /// active when [`MAINTENANCE_MARKER`] exists relative to the process
    /// working directory. The `database` driver is **not** probed because
    /// [`AppConfig`] holds no connection; it always reports `false` and callers
    /// that use it should query their store directly.
    pub fn is_maintenance(&self) -> bool {
        self.maintenance
            .driver
            .eq_ignore_ascii_case(DEFAULT_MAINTENANCE_DRIVER)
            && std::path::Path::new(MAINTENANCE_MARKER).exists()
    }
}

/// Flat serde mirror of the `app_*` keys in `config/app.toml`.
///
/// Deserialized from the whole loader so unknown keys (the other config tables
/// and unrelated environment variables) are ignored. Every field is optional
/// via `#[serde(default = …)]`, so an empty configuration yields Laravel's
/// defaults rather than an error.
#[derive(Debug, Clone, Deserialize)]
struct AppConfigFile {
    /// `app_name` — see [`AppConfig::name`].
    #[serde(default = "default_name")]
    app_name: String,
    /// `app_env` — see [`AppConfig::env`].
    #[serde(default = "default_env")]
    app_env: String,
    /// `app_debug` — see [`AppConfig::debug`].
    #[serde(default)]
    app_debug: bool,
    /// `app_url` — see [`AppConfig::url`].
    #[serde(default = "default_url")]
    app_url: String,
    /// `app_timezone` — see [`AppConfig::timezone`].
    #[serde(default = "default_timezone")]
    app_timezone: String,
    /// `app_locale` — see [`AppConfig::locale`].
    #[serde(default = "default_locale")]
    app_locale: String,
    /// `app_fallback_locale` — see [`AppConfig::fallback_locale`].
    #[serde(default = "default_fallback_locale")]
    app_fallback_locale: String,
    /// `app_faker_locale` — see [`AppConfig::faker_locale`].
    #[serde(default = "default_faker_locale")]
    app_faker_locale: String,
    /// `app_cipher` — see [`AppConfig::cipher`].
    #[serde(default = "default_cipher")]
    app_cipher: String,
    /// `app_key` — see [`AppConfig::key`].
    #[serde(default)]
    app_key: Option<String>,
    /// `app_previous_keys` — see [`AppConfig::previous_keys`].
    #[serde(default)]
    app_previous_keys: Vec<String>,
    /// `app_maintenance_driver` — see [`MaintenanceConfig::driver`].
    #[serde(default = "default_maintenance_driver")]
    app_maintenance_driver: String,
    /// `app_maintenance_store` — see [`MaintenanceConfig::store`].
    #[serde(default = "default_maintenance_store")]
    app_maintenance_store: String,
}

impl From<AppConfigFile> for AppConfig {
    /// Map the flat file shape onto the typed configuration.
    ///
    /// A blank `app_key` is normalised to `None`, matching Laravel's treatment
    /// of an unset `APP_KEY`.
    fn from(file: AppConfigFile) -> Self {
        let key = file.app_key.filter(|value| !value.trim().is_empty());
        Self {
            name: file.app_name,
            env: file.app_env,
            debug: file.app_debug,
            url: file.app_url,
            timezone: file.app_timezone,
            locale: file.app_locale,
            fallback_locale: file.app_fallback_locale,
            faker_locale: file.app_faker_locale,
            cipher: file.app_cipher,
            key,
            previous_keys: file.app_previous_keys,
            maintenance: MaintenanceConfig {
                driver: file.app_maintenance_driver,
                store: file.app_maintenance_store,
            },
        }
    }
}

/// Serde default for [`AppConfig::name`].
fn default_name() -> String {
    DEFAULT_NAME.to_string()
}

/// Serde default for [`AppConfig::env`].
fn default_env() -> String {
    DEFAULT_ENV.to_string()
}

/// Serde default for [`AppConfig::url`].
fn default_url() -> String {
    "http://localhost".to_string()
}

/// Serde default for [`AppConfig::timezone`].
fn default_timezone() -> String {
    DEFAULT_TIMEZONE.to_string()
}

/// Serde default for [`AppConfig::locale`].
fn default_locale() -> String {
    DEFAULT_LOCALE.to_string()
}

/// Serde default for [`AppConfig::fallback_locale`].
fn default_fallback_locale() -> String {
    DEFAULT_FALLBACK_LOCALE.to_string()
}

/// Serde default for [`AppConfig::faker_locale`].
fn default_faker_locale() -> String {
    DEFAULT_FAKER_LOCALE.to_string()
}

/// Serde default for [`AppConfig::cipher`].
fn default_cipher() -> String {
    DEFAULT_CIPHER.to_string()
}

/// Serde default for [`MaintenanceConfig::driver`].
fn default_maintenance_driver() -> String {
    DEFAULT_MAINTENANCE_DRIVER.to_string()
}

/// Serde default for [`MaintenanceConfig::store`].
fn default_maintenance_store() -> String {
    DEFAULT_MAINTENANCE_STORE.to_string()
}
