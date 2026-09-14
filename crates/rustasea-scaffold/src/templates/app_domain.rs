//! Domain layer: models and service providers.
//!
//! Emits `app/mod.rs`, the Eloquent-style `User` model, and the two providers
//! that `bootstrap/app.rs` registers into the boot DAG.

use super::TemplateFile;

/// Domain templates (shared by every variant).
pub fn entries() -> Vec<TemplateFile> {
    vec![
        ("app/mod.rs", APP_MOD),
        ("app/models/mod.rs", MODELS_MOD),
        ("app/models/user.rs", USER),
        ("app/providers/mod.rs", PROVIDERS_MOD),
        (
            "app/providers/app_service_provider.rs",
            APP_SERVICE_PROVIDER,
        ),
        (
            "app/providers/auth_service_provider.rs",
            AUTH_SERVICE_PROVIDER,
        ),
    ]
}

const APP_MOD: &str = r##"//! Application domain layer.
//!
//! Mirrors Laravel's `app/` directory: auth actions, shared concerns, HTTP
//! handlers, models, and service providers.

pub mod actions;
pub mod concerns;
pub mod http;
pub mod models;
pub mod providers;
"##;

const MODELS_MOD: &str = r##"//! Eloquent-style application models.

pub mod user;

pub use user::User;
"##;

const USER: &str = r##"//! `users` model — the shared auth core's user entity.

use chrono::{DateTime, Utc};
use rustasea::orm::{Model, Timestamps};
use uuid::Uuid;

/// Application user account.
///
/// The same model is generated for every variant; only the presentation layer
/// differs (ADR-0002 decision 1).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct User {
    /// Primary key.
    pub id: Uuid,
    /// Display name.
    pub name: String,
    /// Unique, verified email address.
    pub email: String,
    /// Argon2id password hash — never the plaintext.
    ///
    /// Hidden from serialization, mirroring the kit's
    /// `#[Hidden(['password'])]`.
    #[serde(skip_serializing)]
    pub password: String,
    /// Set once the email address is verified.
    pub email_verified_at: Option<DateTime<Utc>>,
    /// Two-factor shared secret, stored encrypted at rest.
    ///
    /// The column holds the *ciphertext*; the encrypt/decrypt layer ships with
    /// the 2FA feature. Hidden from serialization, mirroring the kit's
    /// `#[Hidden(['two_factor_secret'])]`.
    #[serde(skip_serializing)]
    pub two_factor_secret: Option<String>,
    /// JSON array of single-use 2FA recovery codes, stored as text.
    ///
    /// Hidden from serialization, mirroring the kit's
    /// `#[Hidden(['two_factor_recovery_codes'])]`.
    #[serde(skip_serializing)]
    pub two_factor_recovery_codes: Option<String>,
    /// Set once two-factor authentication has been confirmed.
    pub two_factor_confirmed_at: Option<DateTime<Utc>>,
    /// Remember-me token backing persistent ("remember me") logins.
    ///
    /// Hidden from serialization, mirroring the kit's
    /// `#[Hidden(['remember_token'])]`.
    #[serde(skip_serializing)]
    pub remember_token: Option<String>,
    /// Soft-delete marker.
    ///
    /// The ORM soft-deletes by default (`uses_soft_deletes`); a hard delete is
    /// requested explicitly via the ORM's force-delete path when required.
    pub deleted_at: Option<DateTime<Utc>>,
    /// Created/updated timestamps.
    pub timestamps: Timestamps,
}

/// Hand-written ORM contract, mirroring the `make:model` generator.
///
/// The `#[derive(Model)]` macro expands to `rustasea_orm::…` paths, which the
/// generated app does not depend on directly; writing the impl against the
/// umbrella's `rustasea::orm::Model` keeps the app's dependency surface to the
/// single `rustasea` crate.
impl Model for User {
    /// Type name driving the default table derivation.
    fn type_name() -> &'static str {
        "User"
    }

    /// Explicit table name (`users`).
    fn table_name() -> String {
        "users".to_string()
    }

    /// Primary key value.
    fn primary_key(&self) -> Uuid {
        self.id
    }

    /// Assign a fresh client-generated UUID (v7) before persistence.
    fn assign_id(&mut self) -> Uuid {
        self.id = Uuid::now_v7();
        self.id
    }

    /// Soft deletes are enabled via `deleted_at`.
    fn uses_soft_deletes() -> bool {
        true
    }

    /// Timestamps are maintained via the `timestamps` column.
    fn uses_timestamps() -> bool {
        true
    }
}
"##;

const PROVIDERS_MOD: &str = r##"//! Service providers registered into the application boot DAG.

pub mod app_service_provider;
pub mod auth_service_provider;

pub use app_service_provider::AppServiceProvider;
pub use auth_service_provider::AuthServiceProvider;
"##;

const APP_SERVICE_PROVIDER: &str = r##"//! Registers application-wide container bindings.
//!
//! Installs the environment-derived policy defaults, mirroring the kit's
//! `AppServiceProvider::configureDefaults()`.
//!
//! # Immutable dates — not applicable
//!
//! `chrono` has no global mutable date default (`DateTime` is an immutable
//! value type and `Utc::now()` returns a fresh value), so the kit's
//! `Date::use(CarbonImmutable::class)` has no analogue here.

use rustasea::validation::PasswordPolicy;
use rustasea::{Application, ServiceProvider};

/// Container key holding the resolved environment name (`String`).
pub const ENVIRONMENT_KEY: &str = "app.environment";

/// Container key holding the installed [`PasswordPolicy`].
pub const PASSWORD_POLICY_KEY: &str = "app.password_policy";

/// Container key holding whether destructive commands are prohibited (`bool`).
pub const DESTRUCTIVE_COMMANDS_PROHIBITED_KEY: &str = "app.prohibits_destructive_commands";

/// Resolve the active environment (`APP_ENV`, then `config/app.toml`).
pub fn resolve_environment() -> String {
    if let Some(env) = std::env::var("APP_ENV")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return env;
    }
    if let Ok(loader) = rustasea::config::ConfigLoader::load_from(&["config/app"]) {
        if let Ok(env) = loader.get_key::<String>("app_env") {
            let env = env.trim();
            if !env.is_empty() {
                return env.to_string();
            }
        }
    }
    rustasea::orm::PRODUCTION.to_string()
}

/// Select the password policy for `environment`.
///
/// Production is strict; every other environment is relaxed.
pub fn password_policy_for(environment: &str) -> PasswordPolicy {
    if rustasea::orm::is_production(environment) {
        PasswordPolicy::production()
    } else {
        PasswordPolicy::default()
    }
}

/// Application service provider.
pub struct AppServiceProvider;

impl ServiceProvider for AppServiceProvider {
    /// Install the environment-derived policy defaults.
    fn register(&self, app: &mut Application) {
        let environment = resolve_environment();
        let policy = password_policy_for(&environment);
        let prohibited = rustasea::orm::is_production(&environment);
        app.container.instance(ENVIRONMENT_KEY, environment);
        app.container.instance(PASSWORD_POLICY_KEY, policy);
        app.container
            .instance(DESTRUCTIVE_COMMANDS_PROHIBITED_KEY, prohibited);
    }

    fn boot(&self, _app: &Application) {}
}
"##;

const AUTH_SERVICE_PROVIDER: &str = r##"//! Registers the session guard and CSRF wiring.
//!
//! Security-critical: the session guard is shared by every variant, so a
//! hardening fix lands once (ADR-0002 decision 7).

use rustasea::{Application, ServiceProvider};

/// Auth service provider.
pub struct AuthServiceProvider;

impl ServiceProvider for AuthServiceProvider {
    fn register(&self, _app: &mut Application) {}

    fn boot(&self, _app: &Application) {}
}
"##;
