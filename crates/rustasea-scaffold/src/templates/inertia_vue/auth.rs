//! Vue (Leptos) authentication page templates.
//!
//! Each page mirrors the matching blade `resources/views/auth/*.html` contract:
//! the same form action, field names, submit label, and cross-links.

/// Login screen: an interactive credential form plus links to register and to
/// the forgot-password flow.
pub const AUTH_LOGIN: &str = r##"//! `auth/login` Inertia component.

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

/// Registration screen: a name/email/password form with confirmation and a
/// link back to the login screen.
pub const AUTH_REGISTER: &str = r##"//! `auth/register` Inertia component.

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

/// Forgot-password screen: an email form that requests a reset link.
pub const AUTH_FORGOT_PASSWORD: &str = r##"//! `auth/forgot-password` Inertia component.

use leptos::prelude::*;
use rustasea_inertia_adapters::{ClientError, Value};

/// Mount the forgot-password screen with the Inertia page props.
pub fn mount(_props: &Value) -> Result<(), ClientError> {
    mount_to_body(ForgotPassword);
    Ok(())
}

/// Forgot-password screen: an email form that requests a reset link, plus a
/// link back to the login screen.
#[component]
fn ForgotPassword() -> impl IntoView {
    view! {
        <h1>"Forgot password"</h1>
        <p>"Enter your email and we will send a reset link."</p>
        <form method="post" action="/forgot-password">
            <label>
                "Email"
                <input type="email" name="email" required/>
            </label>
            <button type="submit">"Email reset link"</button>
        </form>
        <p><a href="/login">"Back to log in"</a></p>
    }
}
"##;

/// Reset-password screen: the token-bearing form that sets a new password.
pub const AUTH_RESET_PASSWORD: &str = r##"//! `auth/reset-password` Inertia component.

use leptos::prelude::*;
use rustasea_inertia_adapters::{ClientError, Value};

/// Mount the reset-password screen with the Inertia page props.
pub fn mount(_props: &Value) -> Result<(), ClientError> {
    mount_to_body(ResetPassword);
    Ok(())
}

/// Reset-password screen: a token-bearing form that sets a new password. The
/// hidden `token` field carries the value the server embedded in the page
/// props; the empty placeholder is replaced during hydration.
#[component]
fn ResetPassword() -> impl IntoView {
    view! {
        <h1>"Reset password"</h1>
        <p>"Choose a new password for your account."</p>
        <form method="post" action="/reset-password">
            <input type="hidden" name="token" value=""/>
            <label>
                "Email"
                <input type="email" name="email" required/>
            </label>
            <label>
                "New password"
                <input type="password" name="password" required/>
            </label>
            <label>
                "Confirm password"
                <input type="password" name="password_confirmation" required/>
            </label>
            <button type="submit">"Reset password"</button>
        </form>
    }
}
"##;

/// Confirm-password screen: the re-authentication gate for secure areas.
pub const AUTH_CONFIRM_PASSWORD: &str = r##"//! `auth/confirm-password` Inertia component.

use leptos::prelude::*;
use rustasea_inertia_adapters::{ClientError, Value};

/// Mount the confirm-password screen with the Inertia page props.
pub fn mount(_props: &Value) -> Result<(), ClientError> {
    mount_to_body(ConfirmPassword);
    Ok(())
}

/// Confirm-password screen: the re-authentication gate that asks for the
/// current password before entering a secure area.
#[component]
fn ConfirmPassword() -> impl IntoView {
    view! {
        <h1>"Confirm password"</h1>
        <p>"This is a secure area. Please confirm your password before continuing."</p>
        <form method="post" action="/confirm-password">
            <label>
                "Password"
                <input type="password" name="password" required/>
            </label>
            <button type="submit">"Confirm"</button>
        </form>
    }
}
"##;

/// Verify-email screen: the resend-verification and logout actions.
pub const AUTH_VERIFY_EMAIL: &str = r##"//! `auth/verify-email` Inertia component.

use leptos::prelude::*;
use rustasea_inertia_adapters::{ClientError, Value};

/// Mount the verify-email screen with the Inertia page props.
pub fn mount(_props: &Value) -> Result<(), ClientError> {
    mount_to_body(VerifyEmail);
    Ok(())
}

/// Verify-email screen: a resend-verification form plus a logout form, mirroring
/// the blade `auth/verify-email.html` actions.
#[component]
fn VerifyEmail() -> impl IntoView {
    view! {
        <h1>"Verify email"</h1>
        <p>"Check your inbox for the verification link we sent you."</p>
        <form method="post" action="/email/verification-notification">
            <button type="submit">"Resend verification email"</button>
        </form>
        <form method="post" action="/logout">
            <button type="submit">"Log out"</button>
        </form>
    }
}
"##;

/// Two-factor challenge screen: the TOTP or recovery-code gate.
pub const AUTH_TWO_FACTOR_CHALLENGE: &str = r##"//! `auth/two-factor-challenge` Inertia component.

use leptos::prelude::*;
use rustasea_inertia_adapters::{ClientError, Value};

/// Mount the two-factor challenge screen with the Inertia page props.
pub fn mount(_props: &Value) -> Result<(), ClientError> {
    mount_to_body(TwoFactorChallenge);
    Ok(())
}

/// Two-factor challenge screen: the second-factor gate that accepts either a
/// TOTP code or a one-time recovery code.
#[component]
fn TwoFactorChallenge() -> impl IntoView {
    view! {
        <h1>"Two-factor challenge"</h1>
        <p>"Confirm access to your account by entering the authentication code."</p>
        <form method="post" action="/two-factor-challenge">
            <label>
                "Code"
                <input type="text" name="code"/>
            </label>
            <label>
                "Recovery code"
                <input type="text" name="recovery_code"/>
            </label>
            <button type="submit">"Verify"</button>
        </form>
    }
}
"##;
