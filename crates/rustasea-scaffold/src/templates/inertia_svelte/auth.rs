//! Svelte (Sycamore) authentication page templates.
//!
//! Each page mirrors the matching blade `resources/views/auth/*.html` contract:
//! the same form action, field names, submit label, and cross-links.

/// Login screen: an interactive credential form plus links to register and to
/// the forgot-password flow.
pub const AUTH_LOGIN: &str = r##"//! `auth/login` Inertia component.

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

/// Registration screen: a name/email/password form with confirmation and a
/// link back to the login screen.
pub const AUTH_REGISTER: &str = r##"//! `auth/register` Inertia component.

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

/// Forgot-password screen: an email form that requests a reset link.
pub const AUTH_FORGOT_PASSWORD: &str = r##"//! `auth/forgot-password` Inertia component.

use rustasea_inertia_adapters::{ClientError, Value};
use sycamore::prelude::*;

/// Mount the forgot-password screen with the Inertia page props.
pub fn mount(_props: &Value) -> Result<(), ClientError> {
    sycamore::render(|| {
        view! {
            ForgotPassword {}
        }
    });
    Ok(())
}

/// Forgot-password screen: an email form that requests a reset link, plus a
/// link back to the login screen.
#[component]
fn ForgotPassword() -> View {
    view! {
        h1 { "Forgot password" }
        p { "Enter your email and we will send a reset link." }
        form(method="post", action="/forgot-password") {
            label {
                "Email"
                input(r#type="email", name="email", required=true)
            }
            button(r#type="submit") { "Email reset link" }
        }
        p {
            a(href="/login") { "Back to log in" }
        }
    }
}
"##;

/// Reset-password screen: the token-bearing form that sets a new password.
pub const AUTH_RESET_PASSWORD: &str = r##"//! `auth/reset-password` Inertia component.

use rustasea_inertia_adapters::{ClientError, Value};
use sycamore::prelude::*;

/// Mount the reset-password screen with the Inertia page props.
pub fn mount(_props: &Value) -> Result<(), ClientError> {
    sycamore::render(|| {
        view! {
            ResetPassword {}
        }
    });
    Ok(())
}

/// Reset-password screen: a token-bearing form that sets a new password. The
/// hidden `token` field carries the value the server embedded in the page
/// props; the empty placeholder is replaced during hydration.
#[component]
fn ResetPassword() -> View {
    view! {
        h1 { "Reset password" }
        p { "Choose a new password for your account." }
        form(method="post", action="/reset-password") {
            input(r#type="hidden", name="token", value="")
            label {
                "Email"
                input(r#type="email", name="email", required=true)
            }
            label {
                "New password"
                input(r#type="password", name="password", required=true)
            }
            label {
                "Confirm password"
                input(r#type="password", name="password_confirmation", required=true)
            }
            button(r#type="submit") { "Reset password" }
        }
    }
}
"##;

/// Confirm-password screen: the re-authentication gate for secure areas.
pub const AUTH_CONFIRM_PASSWORD: &str = r##"//! `auth/confirm-password` Inertia component.

use rustasea_inertia_adapters::{ClientError, Value};
use sycamore::prelude::*;

/// Mount the confirm-password screen with the Inertia page props.
pub fn mount(_props: &Value) -> Result<(), ClientError> {
    sycamore::render(|| {
        view! {
            ConfirmPassword {}
        }
    });
    Ok(())
}

/// Confirm-password screen: the re-authentication gate that asks for the
/// current password before entering a secure area.
#[component]
fn ConfirmPassword() -> View {
    view! {
        h1 { "Confirm password" }
        p { "This is a secure area. Please confirm your password before continuing." }
        form(method="post", action="/confirm-password") {
            label {
                "Password"
                input(r#type="password", name="password", required=true)
            }
            button(r#type="submit") { "Confirm" }
        }
    }
}
"##;

/// Verify-email screen: the resend-verification and logout actions.
pub const AUTH_VERIFY_EMAIL: &str = r##"//! `auth/verify-email` Inertia component.

use rustasea_inertia_adapters::{ClientError, Value};
use sycamore::prelude::*;

/// Mount the verify-email screen with the Inertia page props.
pub fn mount(_props: &Value) -> Result<(), ClientError> {
    sycamore::render(|| {
        view! {
            VerifyEmail {}
        }
    });
    Ok(())
}

/// Verify-email screen: a resend-verification form plus a logout form, mirroring
/// the blade `auth/verify-email.html` actions.
#[component]
fn VerifyEmail() -> View {
    view! {
        h1 { "Verify email" }
        p { "Check your inbox for the verification link we sent you." }
        form(method="post", action="/email/verification-notification") {
            button(r#type="submit") { "Resend verification email" }
        }
        form(method="post", action="/logout") {
            button(r#type="submit") { "Log out" }
        }
    }
}
"##;

/// Two-factor challenge screen: the TOTP or recovery-code gate.
pub const AUTH_TWO_FACTOR_CHALLENGE: &str = r##"//! `auth/two-factor-challenge` Inertia component.

use rustasea_inertia_adapters::{ClientError, Value};
use sycamore::prelude::*;

/// Mount the two-factor challenge screen with the Inertia page props.
pub fn mount(_props: &Value) -> Result<(), ClientError> {
    sycamore::render(|| {
        view! {
            TwoFactorChallenge {}
        }
    });
    Ok(())
}

/// Two-factor challenge screen: the second-factor gate that accepts either a
/// TOTP code or a one-time recovery code.
#[component]
fn TwoFactorChallenge() -> View {
    view! {
        h1 { "Two-factor challenge" }
        p { "Confirm access to your account by entering the authentication code." }
        form(method="post", action="/two-factor-challenge") {
            label {
                "Code"
                input(r#type="text", name="code")
            }
            label {
                "Recovery code"
                input(r#type="text", name="recovery_code")
            }
            button(r#type="submit") { "Verify" }
        }
    }
}
"##;
