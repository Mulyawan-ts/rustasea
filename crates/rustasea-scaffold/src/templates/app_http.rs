//! HTTP layer: controllers, middleware, and form requests.
//!
//! The controller/request/middleware set is shared across variants; only the
//! dashboard and Inertia middleware are variant-aware (a Blade app never links
//! the Inertia handler — ADR-0002 zero-cost ergonomics).

use crate::variant::StarterKitVariant;

use super::TemplateFile;

/// HTTP templates for `variant`.
pub fn entries(variant: StarterKitVariant) -> Vec<TemplateFile> {
    let mut files = vec![
        ("app/http/mod.rs", HTTP_MOD),
        ("app/http/controllers/mod.rs", CONTROLLERS_MOD),
        ("app/http/controllers/auth_controller.rs", AUTH_CONTROLLER),
        (
            "app/http/controllers/dashboard_controller.rs",
            dashboard_controller(variant),
        ),
        (
            "app/http/controllers/settings/mod.rs",
            SETTINGS_CONTROLLERS_MOD,
        ),
        (
            "app/http/controllers/settings/profile_controller.rs",
            PROFILE_CONTROLLER,
        ),
        (
            "app/http/controllers/settings/password_controller.rs",
            PASSWORD_CONTROLLER,
        ),
        (
            "app/http/controllers/settings/security_controller.rs",
            SECURITY_CONTROLLER,
        ),
        ("app/http/middleware/mod.rs", middleware_mod(variant)),
        (
            "app/http/middleware/ensure_email_is_verified.rs",
            ENSURE_EMAIL_IS_VERIFIED,
        ),
        ("app/http/requests/mod.rs", REQUESTS_MOD),
        ("app/http/requests/settings/mod.rs", SETTINGS_REQUESTS_MOD),
        (
            "app/http/requests/settings/profile_update_request.rs",
            PROFILE_UPDATE_REQUEST,
        ),
        (
            "app/http/requests/settings/password_update_request.rs",
            PASSWORD_UPDATE_REQUEST,
        ),
    ];
    if variant.uses_inertia() {
        files.push((
            "app/http/middleware/handle_inertia_requests.rs",
            HANDLE_INERTIA_REQUESTS,
        ));
    }
    files
}

/// Select the variant-appropriate dashboard controller.
fn dashboard_controller(variant: StarterKitVariant) -> &'static str {
    if variant.uses_inertia() {
        DASHBOARD_CONTROLLER_INERTIA
    } else {
        DASHBOARD_CONTROLLER_VIEW
    }
}

/// Select the middleware module index, including the Inertia handler only when
/// the variant links Inertia.
fn middleware_mod(variant: StarterKitVariant) -> &'static str {
    if variant.uses_inertia() {
        MIDDLEWARE_MOD_INERTIA
    } else {
        MIDDLEWARE_MOD_VIEW
    }
}

const HTTP_MOD: &str = r##"//! HTTP layer — controllers, middleware, and form requests.

pub mod controllers;
pub mod middleware;
pub mod requests;
"##;

const CONTROLLERS_MOD: &str = r##"//! HTTP controllers.

pub mod auth_controller;
pub mod dashboard_controller;
pub mod settings;
"##;

const AUTH_CONTROLLER: &str = r##"//! Login, registration, logout, and password-reset handlers.
//!
//! The auth flow is identical across variants; only the response the handler
//! builds differs (askama view vs Inertia page).

use axum::response::Response;

/// GET /login — show the login screen.
pub async fn show_login() -> Response {
    todo!("render the login screen for this variant")
}

/// POST /login — authenticate and start the session.
pub async fn login() -> Response {
    todo!("authenticate, rotate the session id, and redirect")
}

/// POST /logout — destroy the session and redirect home.
pub async fn logout() -> Response {
    todo!("destroy the session and redirect to /")
}

/// GET /register — show the registration screen.
pub async fn show_register() -> Response {
    todo!("render the registration screen for this variant")
}

/// POST /register — create the account and authenticate.
pub async fn register() -> Response {
    todo!("create the user, log in, and redirect")
}

/// GET /confirm-password — show the password-confirmation screen.
pub async fn show_confirm_password() -> Response {
    todo!("render the confirm-password screen for this variant")
}

/// POST /confirm-password — re-confirm the current password.
pub async fn confirm_password() -> Response {
    todo!("verify the current password and mark it confirmed for the session")
}
"##;

const DASHBOARD_CONTROLLER_VIEW: &str = r##"//! Dashboard handler for server-rendered variants (blade / livewire).
//!
//! The handler renders `resources/views/dashboard.html` through the registered
//! askama [`ViewEngine`](rustasea::view::ViewEngine).

use axum::response::Response;

/// GET /dashboard — render the authenticated dashboard view.
pub async fn index() -> Response {
    todo!("render resources/views/dashboard.html with shared props")
}
"##;

const DASHBOARD_CONTROLLER_INERTIA: &str = r##"//! Dashboard handler for Inertia variants (react / vue).
//!
//! The handler builds an Inertia `Page` and lets `HandleInertiaRequests`
//! decide between the JSON envelope and the HTML shell.

use axum::response::Response;

/// GET /dashboard — render the `dashboard` Inertia component.
pub async fn index() -> Response {
    todo!("render the dashboard Inertia page with shared props")
}
"##;

const SETTINGS_CONTROLLERS_MOD: &str = r##"//! Settings controllers — profile, password, and security screens.

pub mod password_controller;
pub mod profile_controller;
pub mod security_controller;
"##;

const PROFILE_CONTROLLER: &str = r##"//! Profile settings handlers.

use axum::response::Response;

/// GET /settings/profile — show the profile form.
pub async fn edit() -> Response {
    todo!("render the profile settings screen")
}

/// PATCH /settings/profile — persist profile changes.
pub async fn update() -> Response {
    todo!("validate ProfileUpdateRequest and persist the user")
}
"##;

const PASSWORD_CONTROLLER: &str = r##"//! Password settings handlers.

use axum::response::Response;

/// GET /settings/password — show the password form.
pub async fn edit() -> Response {
    todo!("render the password settings screen")
}

/// PUT /settings/password — rotate the password.
pub async fn update() -> Response {
    todo!("validate PasswordUpdateRequest, re-hash, and persist")
}
"##;

const SECURITY_CONTROLLER: &str = r##"//! Security settings handlers.
//!
//! The security screen is where password and two-factor controls live in the
//! kit; only the screen itself is scaffolded here. The 2FA and passkey flows
//! are intentionally not generated (no backing infrastructure yet).

use axum::response::Response;

/// GET /settings/security — show the security screen.
pub async fn edit() -> Response {
    todo!("render the security settings screen")
}
"##;

const MIDDLEWARE_MOD_VIEW: &str = r##"//! HTTP middleware for server-rendered variants.

pub mod ensure_email_is_verified;
"##;

const MIDDLEWARE_MOD_INERTIA: &str = r##"//! HTTP middleware for Inertia variants.

pub mod ensure_email_is_verified;
pub mod handle_inertia_requests;
"##;

const ENSURE_EMAIL_IS_VERIFIED: &str = r##"//! Rejects authenticated-but-unverified users from protected routes.

use axum::response::Response;

/// Whether the user may pass the `verified` middleware.
pub fn passes(email_verified_at: Option<&str>) -> bool {
    email_verified_at.is_some()
}

/// Middleware entry point (wired by the router once middleware binding lands).
pub async fn handle() -> Response {
    todo!("redirect unverified users to /verify-email")
}
"##;

const HANDLE_INERTIA_REQUESTS: &str = r##"//! Owns the per-request Inertia shared props.
//!
//! Resolves the authenticated user, flash data, and CSRF token into the shared
//! prop map merged into every page (ADR-0002 decision 3).

use rustasea::inertia::Inertia;

/// Register the shared props resolved on every Inertia request.
pub fn share(inertia: &mut Inertia) {
    // `Inertia::share` providers run per request; the authenticated user and
    // flash payload are merged here once the auth context lands.
    let _ = inertia;
}
"##;

const REQUESTS_MOD: &str = r##"//! Form request objects (validation at the HTTP boundary).

pub mod settings;
"##;

const SETTINGS_REQUESTS_MOD: &str = r##"//! Settings form requests.

pub mod password_update_request;
pub mod profile_update_request;
"##;

const PROFILE_UPDATE_REQUEST: &str = r##"//! Validated profile-update input.
//!
//! Implemented against the real validation contract: the payload derives
//! `serde::Deserialize` (required by `Validatable: DeserializeOwned`) and
//! `serde::Serialize` (used to build the JSON value the rules run against),
//! and `Validatable::validate` builds a `Rules` set with the fluent
//! `Rules::field(field, "rule|rule")` grammar. Handlers consume it through the
//! `rustasea::validation::FormRequest<ProfileUpdateRequest>` extractor.

use rustasea::validation::{ErrorBag, Rules, Validatable};

/// Profile update form request.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ProfileUpdateRequest {
    /// New display name.
    pub name: String,
    /// New email address.
    pub email: String,
}

impl Validatable for ProfileUpdateRequest {
    /// Validate `name` and `email` with the implemented rule grammar.
    fn validate(&self) -> Result<(), ErrorBag> {
        let rules = Rules::new()
            .field("name", "required|max:255")
            .field("email", "required|email");
        let data = rustasea::validation::serde_json::to_value(self)
            .map_err(|error| ErrorBag::from_message(error.to_string()))?;
        rules.validate(&data)
    }
}
"##;

const PASSWORD_UPDATE_REQUEST: &str = r##"//! Validated password-update input.
//!
//! Implemented against the real validation contract. The confirmation is
//! modelled as an explicit second field with a plain `required|min:12` rule
//! because the `confirmed` rule does not exist yet (AUTH-005 adds it); the
//! explicit field is the same shape the rule will compare against.

use rustasea::validation::{ErrorBag, Rules, Validatable};

/// Password update form request.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct PasswordUpdateRequest {
    /// Current password, re-checked before rotation.
    pub current_password: String,
    /// New password.
    pub password: String,
    /// Confirmation of the new password.
    pub password_confirmation: String,
}

impl Validatable for PasswordUpdateRequest {
    /// Validate the current/new/confirmation fields.
    fn validate(&self) -> Result<(), ErrorBag> {
        let rules = Rules::new()
            .field("current_password", "required")
            .field("password", "required|min:12")
            // TODO(AUTH-005): replace this explicit field with `confirmed` on
            // `password`, which compares it against `password_confirmation`.
            .field("password_confirmation", "required|min:12");
        let data = rustasea::validation::serde_json::to_value(self)
            .map_err(|error| ErrorBag::from_message(error.to_string()))?;
        rules.validate(&data)
    }
}
"##;
