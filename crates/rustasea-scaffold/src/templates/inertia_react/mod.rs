//! React variant WASM frontend — Dioxus + the shared Inertia contract.
//!
//! The entrypoint installs the generated [`ComponentRegistry`] and launches the
//! Dioxus app; each Inertia component key maps to a page module in
//! `resources/js/pages`.
//!
//! The page bodies live in the [`auth`] and [`settings`] submodules so each
//! source file stays within the 500-line limit (ADR-0009); this module keeps the
//! package manifest, entrypoint, generated registry, and dashboard page.

mod auth;
mod settings;

use super::TemplateFile;

/// React (Dioxus) frontend templates.
pub fn entries() -> Vec<TemplateFile> {
    vec![
        ("resources/js/Cargo.toml", CARGO),
        ("resources/js/main.rs", MAIN),
        ("resources/js/pages/mod.rs", PAGES_MOD),
        ("resources/js/pages/dashboard.rs", DASHBOARD),
        ("resources/js/pages/auth_login.rs", auth::AUTH_LOGIN),
        ("resources/js/pages/auth_register.rs", auth::AUTH_REGISTER),
        (
            "resources/js/pages/auth_forgot_password.rs",
            auth::AUTH_FORGOT_PASSWORD,
        ),
        (
            "resources/js/pages/auth_reset_password.rs",
            auth::AUTH_RESET_PASSWORD,
        ),
        (
            "resources/js/pages/auth_confirm_password.rs",
            auth::AUTH_CONFIRM_PASSWORD,
        ),
        (
            "resources/js/pages/auth_verify_email.rs",
            auth::AUTH_VERIFY_EMAIL,
        ),
        (
            "resources/js/pages/auth_two_factor_challenge.rs",
            auth::AUTH_TWO_FACTOR_CHALLENGE,
        ),
        (
            "resources/js/pages/settings_profile.rs",
            settings::SETTINGS_PROFILE,
        ),
        (
            "resources/js/pages/settings_password.rs",
            settings::SETTINGS_PASSWORD,
        ),
        (
            "resources/js/pages/settings_security.rs",
            settings::SETTINGS_SECURITY,
        ),
    ]
}

const CARGO: &str = r##"[package]
name = "@@app_name@@-ui"
version = "0.1.0"
edition = "2021"
rust-version = "1.88"

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
rustasea-inertia-adapters = { version = "0.1", features = ["react"] }
dioxus = { version = "0.7", features = ["web"] }
wasm-bindgen = "0.2"
serde_json = "1"
"##;

const MAIN: &str = r##"//! Dioxus WASM entry point for the react variant.

use dioxus::prelude::*;
use rustasea_inertia_adapters::{install_registry, DioxusRouterProvider, RouterState};

mod pages;

/// Install the generated registry and launch the Dioxus app.
fn main() {
    install_registry(Box::new(pages::AppRegistry::new()));
    dioxus::launch(App);
}

/// Root component — owns the router state hydrated from `data-page`.
#[component]
fn App() -> Element {
    let state = use_signal(RouterState::new);
    rsx! {
        DioxusRouterProvider { state }
    }
}
"##;

const PAGES_MOD: &str = r##"//! Generated Inertia component registry (Dioxus).
//!
//! WASM has no reflection, so the scaffolder emits this `match` explicitly
//! (ADR-0002 decision 4).

use rustasea_inertia_adapters::inertia_registry;

pub mod auth_confirm_password;
pub mod auth_forgot_password;
pub mod auth_login;
pub mod auth_register;
pub mod auth_reset_password;
pub mod auth_two_factor_challenge;
pub mod auth_verify_email;
pub mod dashboard;
pub mod settings_password;
pub mod settings_profile;
pub mod settings_security;

inertia_registry!(AppRegistry {
    "auth/login" => auth_login::mount,
    "auth/register" => auth_register::mount,
    "auth/forgot-password" => auth_forgot_password::mount,
    "auth/reset-password" => auth_reset_password::mount,
    "auth/confirm-password" => auth_confirm_password::mount,
    "auth/verify-email" => auth_verify_email::mount,
    "auth/two-factor-challenge" => auth_two_factor_challenge::mount,
    "dashboard" => dashboard::mount,
    "settings/profile" => settings_profile::mount,
    "settings/password" => settings_password::mount,
    "settings/security" => settings_security::mount,
});
"##;

const DASHBOARD: &str = r##"//! `dashboard` Inertia component.

use dioxus::prelude::*;
use rustasea_inertia_adapters::{ClientError, Value};

/// Mount the dashboard with the Inertia page props.
pub fn mount(_props: &Value) -> Result<(), ClientError> {
    dioxus::launch(Dashboard);
    Ok(())
}

/// Dashboard screen: header, greeting, a summary card, settings links, and a
/// logout form. Mirrors the blade `dashboard.html` semantics.
#[component]
fn Dashboard() -> Element {
    rsx! {
        header {
            h1 { "Dashboard" }
            p { "Welcome back. Here is a summary of your account." }
        }
        section { class: "card",
            h2 { "Summary" }
            p { "Your recent activity and key metrics appear here." }
        }
        nav {
            a { href: "/settings/profile", "Profile settings" }
            a { href: "/settings/password", "Password settings" }
        }
        form { method: "post", action: "/logout",
            button { r#type: "submit", "Log out" }
        }
    }
}
"##;
