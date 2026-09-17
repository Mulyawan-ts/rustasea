//! React (Dioxus) settings page templates.
//!
//! Each page mirrors the matching blade `resources/views/settings/*.html`
//! contract: the same form action, method tunnel, and field names.

/// Profile settings screen: a `PATCH`-tunnelled form for name/email plus links
/// to the password screen and the dashboard.
pub const SETTINGS_PROFILE: &str = r##"//! `settings/profile` Inertia component.

use dioxus::prelude::*;
use rustasea_inertia_adapters::{ClientError, Value};

/// Mount the profile settings screen with the Inertia page props.
pub fn mount(_props: &Value) -> Result<(), ClientError> {
    dioxus::launch(Profile);
    Ok(())
}

/// Profile settings screen: a `PATCH`-tunnelled form for name/email plus links
/// to the password screen and the dashboard.
#[component]
fn Profile() -> Element {
    rsx! {
        h1 { "Profile" }
        form { method: "post", action: "/settings/profile",
            input { r#type: "hidden", name: "_method", value: "PATCH" }
            label {
                "Name"
                input { r#type: "text", name: "name", required: true }
            }
            label {
                "Email"
                input { r#type: "email", name: "email", required: true }
            }
            button { r#type: "submit", "Save" }
        }
        nav {
            a { href: "/settings/password", "Password settings" }
            a { href: "/dashboard", "Dashboard" }
        }
    }
}
"##;

/// Password settings screen: a `PUT`-tunnelled form for the current and new
/// password plus links to the profile screen and the dashboard.
pub const SETTINGS_PASSWORD: &str = r##"//! `settings/password` Inertia component.

use dioxus::prelude::*;
use rustasea_inertia_adapters::{ClientError, Value};

/// Mount the password settings screen with the Inertia page props.
pub fn mount(_props: &Value) -> Result<(), ClientError> {
    dioxus::launch(Password);
    Ok(())
}

/// Password settings screen: a `PUT`-tunnelled form for the current and new
/// password plus links to the profile screen and the dashboard.
#[component]
fn Password() -> Element {
    rsx! {
        h1 { "Password" }
        form { method: "post", action: "/settings/password",
            input { r#type: "hidden", name: "_method", value: "PUT" }
            label {
                "Current password"
                input { r#type: "password", name: "current_password", required: true }
            }
            label {
                "New password"
                input { r#type: "password", name: "password", required: true }
            }
            label {
                "Confirm password"
                input { r#type: "password", name: "password_confirmation", required: true }
            }
            button { r#type: "submit", "Update password" }
        }
        nav {
            a { href: "/settings/profile", "Profile settings" }
            a { href: "/dashboard", "Dashboard" }
        }
    }
}
"##;

/// Security settings screen: two-factor authentication and passkey management.
///
/// Both sections post to the kit's real management endpoints: 2FA to
/// `/user/two-factor-authentication` (enable/disable) and passkeys to
/// `/user/passkeys` (register). The recovery-code screen is linked rather than
/// inlined so the one-time codes are only shown on their dedicated page.
pub const SETTINGS_SECURITY: &str = r##"//! `settings/security` Inertia component.

use dioxus::prelude::*;
use rustasea_inertia_adapters::{ClientError, Value};

/// Mount the security settings screen with the Inertia page props.
pub fn mount(_props: &Value) -> Result<(), ClientError> {
    dioxus::launch(Security);
    Ok(())
}

/// Security settings screen: enable/disable two-factor authentication, link to
/// the recovery codes, and register a passkey. Mirrors the blade
/// `settings/security.html` sections.
#[component]
fn Security() -> Element {
    rsx! {
        h1 { "Security" }
        section {
            h2 { "Two-Factor Authentication" }
            p { "Add an extra layer of security to your account using a TOTP authenticator app." }
            form { method: "post", action: "/user/two-factor-authentication",
                button { r#type: "submit", "Enable two-factor authentication" }
            }
            form { method: "post", action: "/user/two-factor-authentication",
                input { r#type: "hidden", name: "_method", value: "DELETE" }
                button { r#type: "submit", "Disable two-factor authentication" }
            }
            p { a { href: "/user/two-factor-recovery-codes", "View recovery codes" } }
        }
        section {
            h2 { "Passkeys" }
            p { "Sign in without a password using a passkey stored on your device." }
            form { method: "post", action: "/user/passkeys",
                button { r#type: "submit", "Register a passkey" }
            }
        }
    }
}
"##;
