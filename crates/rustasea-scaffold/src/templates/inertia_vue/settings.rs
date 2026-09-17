//! Vue (Leptos) settings page templates.
//!
//! Each page mirrors the matching blade `resources/views/settings/*.html`
//! contract: the same form action, method tunnel, and field names.

/// Profile settings screen: a `PATCH`-tunnelled form for name/email plus links
/// to the password screen and the dashboard.
pub const SETTINGS_PROFILE: &str = r##"//! `settings/profile` Inertia component.

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

/// Password settings screen: a `PUT`-tunnelled form for the current and new
/// password plus links to the profile screen and the dashboard.
pub const SETTINGS_PASSWORD: &str = r##"//! `settings/password` Inertia component.

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

/// Security settings screen: two-factor authentication and passkey management.
///
/// Both sections post to the kit's real management endpoints: 2FA to
/// `/user/two-factor-authentication` (enable/disable) and passkeys to
/// `/user/passkeys` (register). The recovery-code screen is linked rather than
/// inlined so the one-time codes are only shown on their dedicated page.
pub const SETTINGS_SECURITY: &str = r##"//! `settings/security` Inertia component.

use leptos::prelude::*;
use rustasea_inertia_adapters::{ClientError, Value};

/// Mount the security settings screen with the Inertia page props.
pub fn mount(_props: &Value) -> Result<(), ClientError> {
    mount_to_body(Security);
    Ok(())
}

/// Security settings screen: enable/disable two-factor authentication, link to
/// the recovery codes, and register a passkey. Mirrors the blade
/// `settings/security.html` sections.
#[component]
fn Security() -> impl IntoView {
    view! {
        <h1>"Security"</h1>
        <section>
            <h2>"Two-Factor Authentication"</h2>
            <p>"Add an extra layer of security to your account using a TOTP authenticator app."</p>
            <form method="post" action="/user/two-factor-authentication">
                <button type="submit">"Enable two-factor authentication"</button>
            </form>
            <form method="post" action="/user/two-factor-authentication">
                <input type="hidden" name="_method" value="DELETE"/>
                <button type="submit">"Disable two-factor authentication"</button>
            </form>
            <p><a href="/user/two-factor-recovery-codes">"View recovery codes"</a></p>
        </section>
        <section>
            <h2>"Passkeys"</h2>
            <p>"Sign in without a password using a passkey stored on your device."</p>
            <form method="post" action="/user/passkeys">
                <button type="submit">"Register a passkey"</button>
            </form>
        </section>
    }
}
"##;
