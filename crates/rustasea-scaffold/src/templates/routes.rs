//! Route tables mirroring Laravel's `routes/{web,auth,settings,console}.php`.
//!
//! Splitting the single legacy `routes/web.rs` into four concern-scoped tables
//! is ADR-0002 decision 9. Every generated table is built with the `rustasea`
//! router DSL (`rustasea::router::Router`) so each route is named for reverse
//! URL resolution and carries its middleware metadata, mirroring the
//! `laravel/livewire-starter-kit` route conventions.
//!
//! The kit routes that depend on infrastructure this scaffolder does not yet
//! generate are intentionally **omitted**, not stubbed: the appearance/theme
//! screen, the passkey `.well-known` endpoint, forgot/reset password,
//! verify-email, and two-factor authentication. When that infrastructure lands
//! those routes can be added here.

use super::TemplateFile;

/// Route templates (shared by every variant).
pub fn entries() -> Vec<TemplateFile> {
    vec![
        ("routes/mod.rs", ROUTES_MOD),
        ("routes/web.rs", WEB),
        ("routes/auth.rs", AUTH),
        ("routes/settings.rs", SETTINGS),
        ("routes/console.rs", CONSOLE),
    ]
}

const ROUTES_MOD: &str = r##"//! Route tables — `web`, `auth`, `settings`, and `console`.
//!
//! Each table registers into one shared [`Router`] via its `register` function;
//! [`router`] then compiles the DSL table to an axum router and threads the
//! shared [`AppState`] through it.

pub mod auth;
// `console` exports CLI command metadata (the `cargo artisan` registry), not
// HTTP routes — it has no `register(&mut Router)` and is deliberately never
// called from [`router`]. It lives here so the module tree mirrors the kit's
// `routes/` directory.
pub mod console;
pub mod settings;
pub mod web;

use std::sync::Arc;

use rustasea::http::AppState;
use rustasea::router::Router;

/// Build the application router from every generated route table.
///
/// The shared [`AppState`] is created once during boot in `main` and threaded
/// in here, so every route table serves the same state instead of each
/// constructing a disconnected one.
pub fn router(state: Arc<AppState>) -> axum::Router {
    let mut table = Router::new();
    register_placeholder_middleware(&mut table);
    web::register(&mut table);
    auth::register(&mut table);
    settings::register(&mut table);

    // Compile the DSL table to axum. Every middleware id the tables declare is
    // registered above, so this cannot fail with `RouteError::UnknownMiddleware`.
    let router = table
        .try_into_axum_router()
        .expect("every referenced middleware id is registered");

    // Handlers take no extractors yet, so the shared state is attached as a
    // request extension rather than through the `State` extractor. Swap this
    // for `.with_state(state)` once handlers consume `State<Arc<AppState>>`.
    router.layer(axum::Extension(state))
}

/// Register the middleware ids referenced by the generated route tables.
///
/// Each id is registered as a **pass-through placeholder**: it satisfies the
/// router's build-time middleware resolution — an id that was never registered
/// is a typed `RouteError::UnknownMiddleware` — but it does **not** enforce
/// anything yet. The route metadata records the intended guard (`auth`,
/// `verified`, `password.confirm`); replace each closure with the real layer
/// (the session guard, `EnsureEmailIsVerified`, and the confirm-password gate)
/// when that enforcement lands.
fn register_placeholder_middleware(table: &mut Router) {
    table.register_middleware("auth", |method_router| method_router);
    table.register_middleware("verified", |method_router| method_router);
    table.register_middleware("password.confirm", |method_router| method_router);
}
"##;

const WEB: &str = r##"//! Web routes — the public landing page and the authenticated dashboard.
//!
//! `home` is public; `dashboard` sits behind the `auth` + `verified` guards,
//! mirroring the kit's `Route::get('dashboard', ...)->middleware(['auth',
//! 'verified'])->name('dashboard')`.

use rustasea::router::Router;

use crate::app::http::controllers::dashboard_controller;

/// Register the web route table.
pub fn register(table: &mut Router) {
    table.get_action("/", welcome).named("home");

    table.group(|group| {
        group.middleware("auth").middleware("verified");
        group
            .get_action("/dashboard", dashboard_controller::index)
            .named("dashboard");
    });
}

/// GET / — public landing page.
async fn welcome() -> axum::response::Response {
    todo!("render the welcome screen for this variant")
}
"##;

const AUTH: &str = r##"//! Auth routes — login, registration, logout, and password confirmation.
//!
//! Mirrors the kit's `routes/auth.php`: every route is named, and the
//! confirm-password screen (`password.confirm`) re-checks the current password
//! before a sensitive action proceeds.

use rustasea::router::Router;

use crate::app::http::controllers::auth_controller;

/// Register the auth route table.
pub fn register(table: &mut Router) {
    table
        .get_action("/login", auth_controller::show_login)
        .named("login");
    table
        .post_action("/login", auth_controller::login)
        .named("login");
    table
        .post_action("/logout", auth_controller::logout)
        .named("logout");
    table
        .get_action("/register", auth_controller::show_register)
        .named("register");
    table
        .post_action("/register", auth_controller::register)
        .named("register");
    table
        .get_action("/confirm-password", auth_controller::show_confirm_password)
        .named("password.confirm");
    table
        .post_action("/confirm-password", auth_controller::confirm_password)
        .named("password.confirm");
}
"##;

const SETTINGS: &str = r##"//! Settings routes — profile, password, and security management.
//!
//! Mirrors the kit's `routes/settings.php`: `/settings` redirects to the profile
//! screen, the profile/password screens sit behind `auth`, and the security
//! screen additionally requires `verified` and a recent `password.confirm`.

use rustasea::router::Router;

use crate::app::http::controllers::settings::{
    password_controller, profile_controller, security_controller,
};

/// Register the settings route table.
pub fn register(table: &mut Router) {
    table.group(|group| {
        group.middleware("auth");

        // `/settings` is a convenience redirect (302) to the profile screen.
        group
            .redirect("/settings", "/settings/profile")
            .named("settings");

        group
            .get_action("/settings/profile", profile_controller::edit)
            .named("profile.edit");
        group
            .patch_action("/settings/profile", profile_controller::update)
            .named("profile.edit");
        group
            .get_action("/settings/password", password_controller::edit)
            .named("password.edit");
        group
            .put_action("/settings/password", password_controller::update)
            .named("password.edit");

        // The security screen is the most sensitive: it requires a verified
        // account *and* a recently confirmed password (kit parity).
        group.group(|security| {
            security.middleware("verified").middleware("password.confirm");
            security
                .get_action("/settings/security", security_controller::edit)
                .named("security.edit");
        });
    });
}
"##;

const CONSOLE: &str = r##"//! Console route registration — the home of the previously-empty command registry.

use crate::bootstrap::commands;

/// Return the console commands registered by the application.
pub fn register() -> Vec<&'static str> {
    commands::commands()
}
"##;
