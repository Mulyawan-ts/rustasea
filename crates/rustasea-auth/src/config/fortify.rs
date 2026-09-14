//! Typed `[fortify]` table (Laravel Fortify `config/fortify.php` parity).
//!
//! [`FortifyConfig`] mirrors Laravel Fortify's `config/fortify.php`. It holds the
//! guard / password-broker selectors and the route surface (`guard`,
//! `passwords`, `username`, `email`, `home`, `prefix`, `middleware`, `views`),
//! the named rate limiters ([`FortifyLimiterConfig`]), the WebAuthn
//! relying-party settings ([`FortifyPasskeysConfig`]), and the feature toggles
//! ([`FortifyFeaturesConfig`]).
//!
//! [`FortifyConfig::from_loader`] deserializes the table from a layered
//! [`rustasea_config::ConfigLoader`], tolerating a missing `[fortify]` table
//! (yielding [`FortifyConfig::default`], matching the loader's missing-file
//! policy) and then applying the documented single-underscore `FORTIFY_*` /
//! `PASSKEYS_USER_HANDLE_SECRET` environment overrides via
//! [`FortifyConfig::apply_env`].
//!
//! # Inert parity surface
//!
//! Two-factor authentication's *config* remains parity-only (the keys parse and
//! validate; the AUTH-016 runtime reads the `window` field). Passkeys are fully
//! wired (AUTH-017): [`FortifyConfig::resolve_passkeys`] derives the WebAuthn
//! relying party from [`rustasea_foundation::AppConfig`] and the app routes
//! consume the resolved value plus `features.passkeys`.

use serde::Deserialize;

use rustasea_config::ConfigLoader;
use rustasea_foundation::AppConfig;

use crate::error::AuthConfigError;

use super::ConfigResult;

/// Default guard name (`web`).
fn default_guard() -> String {
    "web".to_string()
}

/// Default password broker (`users`).
fn default_passwords() -> String {
    "users".to_string()
}

/// Default username request field (`email`).
fn default_username() -> String {
    "email".to_string()
}

/// Default email request field (`email`).
fn default_email() -> String {
    "email".to_string()
}

/// Default post-auth redirect target (`/dashboard`).
fn default_home() -> String {
    "/dashboard".to_string()
}

/// Default route middleware (`["web"]`).
fn default_middleware() -> Vec<String> {
    vec!["web".to_string()]
}

/// Default login limiter name (`login`).
fn default_login_limiter() -> String {
    "login".to_string()
}

/// Default WebAuthn ceremony timeout, in milliseconds (`60000`).
fn default_timeout() -> u64 {
    60_000
}

/// Serde default for the boolean feature toggles (`true`).
fn default_true() -> bool {
    true
}

/// `[fortify.limiters]` — the named rate limiters for the Fortify routes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FortifyLimiterConfig {
    /// Limiter applied to the login route.
    #[serde(default = "default_login_limiter")]
    pub login: String,
    /// Limiter applied to the two-factor challenge route (inert; unset by default).
    #[serde(default, rename = "two-factor")]
    pub two_factor: Option<String>,
    /// Limiter applied to the passkey routes (inert; unset by default).
    #[serde(default)]
    pub passkeys: Option<String>,
}

impl Default for FortifyLimiterConfig {
    /// Laravel defaults: only the `login` limiter is named.
    fn default() -> Self {
        Self {
            login: default_login_limiter(),
            two_factor: None,
            passkeys: None,
        }
    }
}

/// `[fortify.passkeys]` — the WebAuthn relying-party settings (inert parity).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FortifyPasskeysConfig {
    /// WebAuthn relying-party id; derived from `app.url` when unset.
    #[serde(default)]
    pub relying_party_id: Option<String>,
    /// Allowed origins; defaults to `[app.url]` when unset.
    #[serde(default)]
    pub allowed_origins: Option<Vec<String>>,
    /// HMAC key for user handles; falls back to `app.key` when blank.
    #[serde(default)]
    pub user_handle_secret: String,
    /// Ceremony timeout, in milliseconds.
    #[serde(default = "default_timeout")]
    pub timeout: u64,
}

impl Default for FortifyPasskeysConfig {
    /// RustaSea defaults: derive the relying party, no explicit secret.
    fn default() -> Self {
        Self {
            relying_party_id: None,
            allowed_origins: None,
            user_handle_secret: String::new(),
            timeout: default_timeout(),
        }
    }
}

/// `[fortify.features.two_factor_authentication]` (inert parity).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FortifyTwoFactorConfig {
    /// Require password confirmation before enabling two-factor auth.
    #[serde(default = "default_true")]
    pub confirm: bool,
    /// Require password confirmation before disabling two-factor auth.
    #[serde(default = "default_true")]
    pub confirm_password: bool,
    /// Number of previous one-time codes accepted; unset uses the driver default.
    #[serde(default)]
    pub window: Option<u64>,
}

impl Default for FortifyTwoFactorConfig {
    /// Laravel defaults: confirmation required, no pinned window.
    fn default() -> Self {
        Self {
            confirm: true,
            confirm_password: true,
            window: None,
        }
    }
}

/// `[fortify.features.passkeys]`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FortifyPasskeyFeatureConfig {
    /// Enable the passkey routes; disabled routes answer `404`.
    ///
    /// Laravel enables a Fortify feature by listing it in `features`; RustaSea's
    /// static route table cannot be removed at runtime, so this explicit toggle
    /// is the closest honest analogue (defaults to enabled, matching the kit).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Require password confirmation before managing passkeys.
    #[serde(default = "default_true")]
    pub confirm_password: bool,
}

impl Default for FortifyPasskeyFeatureConfig {
    /// Laravel defaults: enabled, password confirmation required.
    fn default() -> Self {
        Self {
            enabled: true,
            confirm_password: true,
        }
    }
}

/// `[fortify.features]` — the built-in auth feature toggles.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FortifyFeaturesConfig {
    /// Allow user registration.
    #[serde(default = "default_true")]
    pub registration: bool,
    /// Allow password resets.
    #[serde(default = "default_true")]
    pub reset_passwords: bool,
    /// Require email verification.
    #[serde(default = "default_true")]
    pub email_verification: bool,
    /// Two-factor authentication settings (inert parity).
    #[serde(default)]
    pub two_factor_authentication: FortifyTwoFactorConfig,
    /// Passkey settings (inert parity).
    #[serde(default)]
    pub passkeys: FortifyPasskeyFeatureConfig,
}

impl Default for FortifyFeaturesConfig {
    /// Laravel defaults: every feature enabled.
    fn default() -> Self {
        Self {
            registration: true,
            reset_passwords: true,
            email_verification: true,
            two_factor_authentication: FortifyTwoFactorConfig::default(),
            passkeys: FortifyPasskeyFeatureConfig::default(),
        }
    }
}

/// Typed `[fortify]` table (Laravel Fortify `config/fortify.php` shape).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FortifyConfig {
    /// Guard used for the Fortify routes (must name an `[auth.guards.*]` entry).
    #[serde(default = "default_guard")]
    pub guard: String,
    /// Password broker used for reset flows (must name an `[auth.passwords.*]`).
    #[serde(default = "default_passwords")]
    pub passwords: String,
    /// Request field carrying the username.
    #[serde(default = "default_username")]
    pub username: String,
    /// Request field carrying the email.
    #[serde(default = "default_email")]
    pub email: String,
    /// Lowercase usernames before lookup.
    #[serde(default = "default_true")]
    pub lowercase_usernames: bool,
    /// Post-auth / post-reset redirect target.
    #[serde(default = "default_home")]
    pub home: String,
    /// Route prefix prepended to every Fortify route.
    #[serde(default)]
    pub prefix: String,
    /// Subdomain the Fortify routes are bound to; `None` means "no subdomain".
    #[serde(default)]
    pub domain: Option<String>,
    /// Middleware applied to the Fortify routes.
    #[serde(default = "default_middleware")]
    pub middleware: Vec<String>,
    /// Serve Fortify's built-in view routes.
    #[serde(default = "default_true")]
    pub views: bool,
    /// Named rate limiters.
    #[serde(default)]
    pub limiters: FortifyLimiterConfig,
    /// WebAuthn relying-party settings (inert parity).
    #[serde(default)]
    pub passkeys: FortifyPasskeysConfig,
    /// Feature toggles.
    #[serde(default)]
    pub features: FortifyFeaturesConfig,
}

impl Default for FortifyConfig {
    /// Laravel Fortify defaults with every feature enabled.
    fn default() -> Self {
        Self {
            guard: default_guard(),
            passwords: default_passwords(),
            username: default_username(),
            email: default_email(),
            lowercase_usernames: true,
            home: default_home(),
            prefix: String::new(),
            domain: None,
            middleware: default_middleware(),
            views: true,
            limiters: FortifyLimiterConfig::default(),
            passkeys: FortifyPasskeysConfig::default(),
            features: FortifyFeaturesConfig::default(),
        }
    }
}

/// Resolved WebAuthn relying-party settings (the [`FortifyConfig::resolve_passkeys`] output).
///
/// Unlike [`FortifyPasskeysConfig`], every field is concrete: the relying-party
/// id and allowed origins are derived from [`AppConfig::url`] when the file
/// leaves them unset, and the user-handle secret falls back to [`AppConfig::key`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPasskeys {
    /// WebAuthn relying-party id (the host of `app.url`).
    pub relying_party_id: String,
    /// Allowed origins (the configured list, or `[app.url]`).
    pub allowed_origins: Vec<String>,
    /// HMAC key for user handles (the configured value, or `app.key`).
    pub user_handle_secret: String,
    /// Ceremony timeout, in milliseconds.
    pub timeout: u64,
}

impl FortifyConfig {
    /// Deserialize `[fortify]` from a layered [`ConfigLoader`] and apply the
    /// documented environment overrides.
    ///
    /// A missing `[fortify]` table yields [`FortifyConfig::default`] (tolerated,
    /// matching the loader's missing-file policy); a table that exists but does
    /// not deserialize surfaces [`AuthConfigError::Invalid`].
    ///
    /// # Errors
    ///
    /// [`AuthConfigError::Invalid`] when `[fortify]` exists but is malformed, or
    /// when a `FORTIFY_*` override is not a recognised boolean.
    pub fn from_loader(loader: &ConfigLoader) -> ConfigResult<Self> {
        let mut config = match loader.get_key::<FortifyConfig>("fortify") {
            Ok(config) => config,
            Err(error) => {
                if loader.inner().get_table("fortify").is_err() {
                    FortifyConfig::default()
                } else {
                    return Err(AuthConfigError::Invalid(error.to_string()));
                }
            }
        };
        config.apply_env()?;
        Ok(config)
    }

    /// Apply the documented single-underscore environment overrides.
    ///
    /// Every `FORTIFY_*` variable replaces its file counterpart and
    /// `PASSKEYS_USER_HANDLE_SECRET` replaces `[fortify.passkeys].user_handle_secret`.
    /// The environment wins over the file and a blank value is ignored; the
    /// loader's `__` separator means single-underscore variables never reach the
    /// nested `[fortify]` table, so this bridge is the only path for `.env`
    /// parity. `FORTIFY_MIDDLEWARE` is comma-separated.
    ///
    /// # Errors
    ///
    /// [`AuthConfigError::Invalid`] when a boolean override
    /// (`FORTIFY_LOWERCASE_USERNAMES`, `FORTIFY_VIEWS`, `FORTIFY_REGISTRATION`,
    /// `FORTIFY_RESET_PASSWORDS`, `FORTIFY_EMAIL_VERIFICATION`) is not a
    /// recognised boolean.
    pub fn apply_env(&mut self) -> ConfigResult<()> {
        if let Some(guard) = env_non_empty("FORTIFY_GUARD") {
            self.guard = guard;
        }
        if let Some(passwords) = env_non_empty("FORTIFY_PASSWORDS") {
            self.passwords = passwords;
        }
        if let Some(username) = env_non_empty("FORTIFY_USERNAME") {
            self.username = username;
        }
        if let Some(email) = env_non_empty("FORTIFY_EMAIL") {
            self.email = email;
        }
        if let Some(lowercase) = env_non_empty("FORTIFY_LOWERCASE_USERNAMES") {
            self.lowercase_usernames = parse_bool("FORTIFY_LOWERCASE_USERNAMES", &lowercase)?;
        }
        if let Some(home) = env_non_empty("FORTIFY_HOME") {
            self.home = home;
        }
        if let Some(prefix) = env_non_empty("FORTIFY_PREFIX") {
            self.prefix = prefix;
        }
        if let Some(domain) = env_non_empty("FORTIFY_DOMAIN") {
            self.domain = Some(domain);
        }
        if let Some(middleware) = env_non_empty("FORTIFY_MIDDLEWARE") {
            self.middleware = middleware
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect();
        }
        if let Some(views) = env_non_empty("FORTIFY_VIEWS") {
            self.views = parse_bool("FORTIFY_VIEWS", &views)?;
        }
        if let Some(login) = env_non_empty("FORTIFY_LIMITERS_LOGIN") {
            self.limiters.login = login;
        }
        if let Some(secret) = env_non_empty("PASSKEYS_USER_HANDLE_SECRET") {
            self.passkeys.user_handle_secret = secret;
        }
        if let Some(registration) = env_non_empty("FORTIFY_REGISTRATION") {
            self.features.registration = parse_bool("FORTIFY_REGISTRATION", &registration)?;
        }
        if let Some(reset) = env_non_empty("FORTIFY_RESET_PASSWORDS") {
            self.features.reset_passwords = parse_bool("FORTIFY_RESET_PASSWORDS", &reset)?;
        }
        if let Some(verification) = env_non_empty("FORTIFY_EMAIL_VERIFICATION") {
            self.features.email_verification =
                parse_bool("FORTIFY_EMAIL_VERIFICATION", &verification)?;
        }
        if let Some(enabled) = env_non_empty("FORTIFY_PASSKEYS_ENABLED") {
            self.features.passkeys.enabled = parse_bool("FORTIFY_PASSKEYS_ENABLED", &enabled)?;
        }
        Ok(())
    }

    /// Resolve the WebAuthn relying-party settings against [`AppConfig`].
    ///
    /// The relying-party id is the host of `app.url` unless the file sets
    /// `relying_party_id`; the allowed origins default to `[app.url]`; and the
    /// user-handle secret falls back to `app.key` when blank. Consumed by the
    /// AUTH-017 passkey service to build registration/authentication ceremonies.
    pub fn resolve_passkeys(&self, app: &AppConfig) -> ResolvedPasskeys {
        let relying_party_id = self
            .passkeys
            .relying_party_id
            .clone()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| host_from_url(&app.url))
            .unwrap_or_default();
        let allowed_origins = self
            .passkeys
            .allowed_origins
            .clone()
            .filter(|origins| !origins.is_empty())
            .unwrap_or_else(|| vec![app.url.clone()]);
        let user_handle_secret = if self.passkeys.user_handle_secret.trim().is_empty() {
            app.key.clone().unwrap_or_default()
        } else {
            self.passkeys.user_handle_secret.clone()
        };
        ResolvedPasskeys {
            relying_party_id,
            allowed_origins,
            user_handle_secret,
            timeout: self.passkeys.timeout,
        }
    }
}

/// Extract the host (without port, userinfo, or brackets) from a URL.
///
/// Deliberately minimal — it handles the `scheme://host[:port][/path]` shapes
/// that `app.url` carries, including bracketed IPv6 literals, without pulling in
/// a full URL parser. Returns `None` when no host can be found.
fn host_from_url(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    if host_port.is_empty() {
        return None;
    }
    if let Some(rest) = host_port.strip_prefix('[') {
        return rest
            .split(']')
            .next()
            .map(str::to_string)
            .filter(|host| !host.is_empty());
    }
    let host = host_port.split(':').next().unwrap_or(host_port);
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

/// Read an environment variable, treating unset or blank values as absent.
fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// Parse a truthy/falsy environment override, mapping an unrecognised value to a
/// typed [`AuthConfigError::Invalid`] that names the offending variable.
fn parse_bool(key: &str, value: &str) -> ConfigResult<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Ok(true),
        "0" | "false" | "off" | "no" => Ok(false),
        _ => Err(AuthConfigError::Invalid(format!(
            "{key} {value:?} is not a recognised boolean"
        ))),
    }
}

#[cfg(test)]
mod tests;
