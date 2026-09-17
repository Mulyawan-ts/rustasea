//! Svelte variant WASM frontend: Sycamore + the shared Inertia contract.
//!
//! The entrypoint installs the generated [`AppRegistry`] and mounts the Sycamore
//! app; each Inertia component key maps to a page module in `resources/js/pages`.
//! The layout mirrors the react (Dioxus) and vue (Leptos) kits so the remaining
//! Inertia pages can be added uniformly.

use super::TemplateFile;

/// Svelte (Sycamore) frontend templates.
pub fn entries() -> Vec<TemplateFile> {
    vec![
        ("resources/js/Cargo.toml", CARGO),
        ("resources/js/main.rs", MAIN),
        ("resources/js/pages/mod.rs", PAGES_MOD),
        ("resources/js/pages/dashboard.rs", DASHBOARD),
        ("resources/js/pages/auth_login.rs", AUTH_LOGIN),
        ("resources/js/pages/auth_register.rs", AUTH_REGISTER),
        ("resources/js/pages/settings_profile.rs", SETTINGS_PROFILE),
        ("resources/js/pages/settings_password.rs", SETTINGS_PASSWORD),
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
rustasea-inertia-adapters = { version = "0.1", features = ["svelte"] }
sycamore = { version = "0.9", features = ["web"] }
wasm-bindgen = "0.2"
serde_json = "1"
"##;

const MAIN: &str = r##"//! Sycamore WASM entry point for the svelte variant.

use rustasea_inertia_adapters::{install_registry, SycamoreRouterProvider};
use sycamore::prelude::*;

mod pages;

/// Install the generated registry and mount the Sycamore app.
///
/// Sycamore 0.9 boots a client-rendered app with [`sycamore::render`] (the
/// hydration counterpart, `sycamore::hydrate`, is reserved for SSR and gated
/// behind the `hydrate` feature). The router provider hydrates the initial page
/// from the JSON the server embedded in the `#app` element's `data-page`
/// attribute and mounts the matching component through the registry.
fn main() {
    install_registry(Box::new(pages::AppRegistry::new()));
    let initial_page = read_initial_page();
    sycamore::render(move || {
        view! {
            SycamoreRouterProvider(initial_page=initial_page) {}
        }
    });
}

/// Read the escaped Inertia page JSON from the `#app` mount element.
///
/// Returns an empty string when the shell carries no page, which the provider
/// treats as "nothing to hydrate".
fn read_initial_page() -> String {
    document()
        .query_selector("#app")
        .ok()
        .flatten()
        .and_then(|element| element.get_attribute("data-page"))
        .unwrap_or_default()
}
"##;

const PAGES_MOD: &str = r##"//! Generated Inertia component registry (Sycamore).
//!
//! WASM has no reflection, so the scaffolder emits this `match` explicitly
//! (ADR-0002 decision 4).

use rustasea_inertia_adapters::inertia_registry;

pub mod auth_login;
pub mod auth_register;
pub mod dashboard;
pub mod settings_password;
pub mod settings_profile;

inertia_registry!(AppRegistry {
    "auth/login" => auth_login::mount,
    "auth/register" => auth_register::mount,
    "dashboard" => dashboard::mount,
    "settings/password" => settings_password::mount,
    "settings/profile" => settings_profile::mount,
});
"##;

const DASHBOARD: &str = r##"//! `dashboard` Inertia component.

use rustasea_inertia_adapters::{ClientError, Value};
use sycamore::prelude::*;

/// Mount the dashboard with the Inertia page props.
pub fn mount(_props: &Value) -> Result<(), ClientError> {
    sycamore::render(|| {
        view! {
            Dashboard {}
        }
    });
    Ok(())
}

/// Dashboard screen: header, greeting, a summary card, settings links, and a
/// logout form. Mirrors the blade `dashboard.html` semantics.
#[component]
fn Dashboard() -> View {
    view! {
        header {
            h1 { "Dashboard" }
            p { "Welcome back. Here is a summary of your account." }
        }
        section(class="card") {
            h2 { "Summary" }
            p { "Your recent activity and key metrics appear here." }
        }
        nav {
            a(href="/settings/profile") { "Profile settings" }
            a(href="/settings/password") { "Password settings" }
        }
        form(method="post", action="/logout") {
            button(r#type="submit") { "Log out" }
        }
    }
}
"##;

const AUTH_LOGIN: &str = r##"//! `auth/login` Inertia component.

use rustasea_inertia_adapters::{ClientError, Value};
use sycamore::prelude::*;

/// Mount the login screen with the Inertia page props.
pub fn mount(_props: &Value) -> Result<(), ClientError> {
    sycamore::render(|| {
        view! {
            Login {}
        }
    });
    Ok(())
}

/// Login screen: an interactive credential form plus links to register and to
/// the forgot-password flow.
#[component]
fn Login() -> View {
    view! {
        h1 { "Log in" }
        form(method="post", action="/login") {
            label {
                "Email"
                input(r#type="email", name="email", required=true)
            }
            label {
                "Password"
                input(r#type="password", name="password", required=true)
            }
            label {
                input(r#type="checkbox", name="remember")
                "Remember me"
            }
            button(r#type="submit") { "Log in" }
        }
        p {
            a(href="/register") { "Don't have an account? Register" }
        }
        p {
            a(href="/forgot-password") { "Forgot your password?" }
        }
    }
}
"##;

const AUTH_REGISTER: &str = r##"//! `auth/register` Inertia component.

use rustasea_inertia_adapters::{ClientError, Value};
use sycamore::prelude::*;

/// Mount the registration screen with the Inertia page props.
pub fn mount(_props: &Value) -> Result<(), ClientError> {
    sycamore::render(|| {
        view! {
            Register {}
        }
    });
    Ok(())
}

/// Registration screen: a name/email/password form with confirmation and a
/// link back to the login screen.
#[component]
fn Register() -> View {
    view! {
        h1 { "Register" }
        form(method="post", action="/register") {
            label {
                "Name"
                input(r#type="text", name="name", required=true)
            }
            label {
                "Email"
                input(r#type="email", name="email", required=true)
            }
            label {
                "Password"
                input(r#type="password", name="password", required=true)
            }
            label {
                "Confirm password"
                input(r#type="password", name="password_confirmation", required=true)
            }
            button(r#type="submit") { "Register" }
        }
        p {
            a(href="/login") { "Already registered? Log in" }
        }
    }
}
"##;

const SETTINGS_PROFILE: &str = r##"//! `settings/profile` Inertia component.

use rustasea_inertia_adapters::{ClientError, Value};
use sycamore::prelude::*;

/// Mount the profile settings screen with the Inertia page props.
pub fn mount(_props: &Value) -> Result<(), ClientError> {
    sycamore::render(|| {
        view! {
            Profile {}
        }
    });
    Ok(())
}

/// Profile settings screen: a `PATCH`-tunnelled form for name/email plus links
/// to the password screen and the dashboard.
#[component]
fn Profile() -> View {
    view! {
        h1 { "Profile" }
        form(method="post", action="/settings/profile") {
            input(r#type="hidden", name="_method", value="PATCH")
            label {
                "Name"
                input(r#type="text", name="name", required=true)
            }
            label {
                "Email"
                input(r#type="email", name="email", required=true)
            }
            button(r#type="submit") { "Save" }
        }
        nav {
            a(href="/settings/password") { "Password settings" }
            a(href="/dashboard") { "Dashboard" }
        }
    }
}
"##;

const SETTINGS_PASSWORD: &str = r##"//! `settings/password` Inertia component.

use rustasea_inertia_adapters::{ClientError, Value};
use sycamore::prelude::*;

/// Mount the password settings screen with the Inertia page props.
pub fn mount(_props: &Value) -> Result<(), ClientError> {
    sycamore::render(|| {
        view! {
            Password {}
        }
    });
    Ok(())
}

/// Password settings screen: a `PUT`-tunnelled form for the current and new
/// password plus links to the profile screen and the dashboard.
#[component]
fn Password() -> View {
    view! {
        h1 { "Password" }
        form(method="post", action="/settings/password") {
            input(r#type="hidden", name="_method", value="PUT")
            label {
                "Current password"
                input(r#type="password", name="current_password", required=true)
            }
            label {
                "New password"
                input(r#type="password", name="password", required=true)
            }
            label {
                "Confirm password"
                input(r#type="password", name="password_confirmation", required=true)
            }
            button(r#type="submit") { "Update password" }
        }
        nav {
            a(href="/settings/profile") { "Profile settings" }
            a(href="/dashboard") { "Dashboard" }
        }
    }
}
"##;
