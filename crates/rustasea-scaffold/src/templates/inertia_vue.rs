//! Vue variant WASM frontend — Leptos + the shared Inertia contract.
//!
//! The entrypoint installs the generated [`ComponentRegistry`] and mounts the
//! Leptos app; each Inertia component key maps to a page module in
//! `resources/js/pages`.

use super::TemplateFile;

/// Vue (Leptos) frontend templates.
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
rustasea-inertia-adapters = { version = "0.1", features = ["vue"] }
leptos = { version = "0.8", features = ["csr"] }
wasm-bindgen = "0.2"
serde_json = "1"
"##;

const MAIN: &str = r##"//! Leptos WASM entry point for the vue variant.

use leptos::prelude::*;
use rustasea_inertia_adapters::{install_registry, LeptosRouterProvider, RouterState};

mod pages;

/// Install the generated registry and mount the Leptos app.
fn main() {
    install_registry(Box::new(pages::AppRegistry::new()));
    mount_to_body(App);
}

/// Root component — owns the router state hydrated from `data-page`.
#[component]
fn App() -> impl IntoView {
    let state = RouterState::new();
    view! {
        <LeptosRouterProvider state=state/>
    }
}
"##;

const PAGES_MOD: &str = r##"//! Generated Inertia component registry (Leptos).
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

use leptos::prelude::*;
use rustasea_inertia_adapters::{ClientError, Value};

/// Mount the dashboard with the Inertia page props.
pub fn mount(_props: &Value) -> Result<(), ClientError> {
    mount_to_body(Dashboard);
    Ok(())
}

/// Dashboard screen: header, greeting, a summary card, settings links, and a
/// logout form. Mirrors the blade `dashboard.html` semantics.
#[component]
fn Dashboard() -> impl IntoView {
    view! {
        <header>
            <h1>"Dashboard"</h1>
            <p>"Welcome back. Here is a summary of your account."</p>
        </header>
        <section class="card">
            <h2>"Summary"</h2>
            <p>"Your recent activity and key metrics appear here."</p>
        </section>
        <nav>
            <a href="/settings/profile">"Profile settings"</a>
            <a href="/settings/password">"Password settings"</a>
        </nav>
        <form method="post" action="/logout">
            <button type="submit">"Log out"</button>
        </form>
    }
}
"##;

const AUTH_LOGIN: &str = r##"//! `auth/login` Inertia component.

use leptos::prelude::*;
use rustasea_inertia_adapters::{ClientError, Value};

/// Mount the login screen with the Inertia page props.
pub fn mount(_props: &Value) -> Result<(), ClientError> {
    mount_to_body(Login);
    Ok(())
}

/// Login screen: an interactive credential form plus links to register and to
/// the forgot-password flow.
#[component]
fn Login() -> impl IntoView {
    view! {
        <h1>"Log in"</h1>
        <form method="post" action="/login">
            <label>
                "Email"
                <input type="email" name="email" required/>
            </label>
            <label>
                "Password"
                <input type="password" name="password" required/>
            </label>
            <label>
                <input type="checkbox" name="remember"/>
                "Remember me"
            </label>
            <button type="submit">"Log in"</button>
        </form>
        <p><a href="/register">"Don't have an account? Register"</a></p>
        <p><a href="/forgot-password">"Forgot your password?"</a></p>
    }
}
"##;

const AUTH_REGISTER: &str = r##"//! `auth/register` Inertia component.

use leptos::prelude::*;
use rustasea_inertia_adapters::{ClientError, Value};

/// Mount the registration screen with the Inertia page props.
pub fn mount(_props: &Value) -> Result<(), ClientError> {
    mount_to_body(Register);
    Ok(())
}

/// Registration screen: a name/email/password form with confirmation and a
/// link back to the login screen.
#[component]
fn Register() -> impl IntoView {
    view! {
        <h1>"Register"</h1>
        <form method="post" action="/register">
            <label>
                "Name"
                <input type="text" name="name" required/>
            </label>
            <label>
                "Email"
                <input type="email" name="email" required/>
            </label>
            <label>
                "Password"
                <input type="password" name="password" required/>
            </label>
            <label>
                "Confirm password"
                <input type="password" name="password_confirmation" required/>
            </label>
            <button type="submit">"Register"</button>
        </form>
        <p><a href="/login">"Already registered? Log in"</a></p>
    }
}
"##;

const SETTINGS_PROFILE: &str = r##"//! `settings/profile` Inertia component.

use leptos::prelude::*;
use rustasea_inertia_adapters::{ClientError, Value};

/// Mount the profile settings screen with the Inertia page props.
pub fn mount(_props: &Value) -> Result<(), ClientError> {
    mount_to_body(Profile);
    Ok(())
}

/// Profile settings screen: a `PATCH`-tunnelled form for name/email plus links
/// to the password screen and the dashboard.
#[component]
fn Profile() -> impl IntoView {
    view! {
        <h1>"Profile"</h1>
        <form method="post" action="/settings/profile">
            <input type="hidden" name="_method" value="PATCH"/>
            <label>
                "Name"
                <input type="text" name="name" required/>
            </label>
            <label>
                "Email"
                <input type="email" name="email" required/>
            </label>
            <button type="submit">"Save"</button>
        </form>
        <nav>
            <a href="/settings/password">"Password settings"</a>
            <a href="/dashboard">"Dashboard"</a>
        </nav>
    }
}
"##;

const SETTINGS_PASSWORD: &str = r##"//! `settings/password` Inertia component.

use leptos::prelude::*;
use rustasea_inertia_adapters::{ClientError, Value};

/// Mount the password settings screen with the Inertia page props.
pub fn mount(_props: &Value) -> Result<(), ClientError> {
    mount_to_body(Password);
    Ok(())
}

/// Password settings screen: a `PUT`-tunnelled form for the current and new
/// password plus links to the profile screen and the dashboard.
#[component]
fn Password() -> impl IntoView {
    view! {
        <h1>"Password"</h1>
        <form method="post" action="/settings/password">
            <input type="hidden" name="_method" value="PUT"/>
            <label>
                "Current password"
                <input type="password" name="current_password" required/>
            </label>
            <label>
                "New password"
                <input type="password" name="password" required/>
            </label>
            <label>
                "Confirm password"
                <input type="password" name="password_confirmation" required/>
            </label>
            <button type="submit">"Update password"</button>
        </form>
        <nav>
            <a href="/settings/profile">"Profile settings"</a>
            <a href="/dashboard">"Dashboard"</a>
        </nav>
    }
}
"##;
