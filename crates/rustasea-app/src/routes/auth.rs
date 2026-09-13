//! Auth routes — login, logout, registration, and password confirmation.
//!
//! The GET routes render minimal placeholder pages and the POST routes answer
//! `501 Not Implemented` with a clear message. No login is faked: the flows
//! are explicitly unimplemented so a caller cannot mistake the scaffold for a
//! working authentication surface.

use axum::response::{Html, Response};

use rustasea::router::Router as RouteTable;

use super::not_implemented;

/// Login page markup (placeholder form).
const LOGIN_HTML: &str = r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><title>Log in</title></head>
<body><main><h1>Log in</h1>
<form method="post" action="/login">
<label>Email <input type="email" name="email" required></label>
<label>Password <input type="password" name="password" required></label>
<button type="submit">Log in</button>
</form>
<p><a href="/register">Create an account</a></p>
</main></body></html>"#;

/// Registration page markup (placeholder form).
const REGISTER_HTML: &str = r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><title>Register</title></head>
<body><main><h1>Register</h1>
<form method="post" action="/register">
<label>Name <input type="text" name="name" required></label>
<label>Email <input type="email" name="email" required></label>
<label>Password <input type="password" name="password" required></label>
<button type="submit">Register</button>
</form>
<p><a href="/login">Already registered?</a></p>
</main></body></html>"#;

/// Password-confirmation page markup (placeholder form).
const CONFIRM_HTML: &str = r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><title>Confirm password</title></head>
<body><main><h1>Confirm password</h1>
<form method="post" action="/confirm-password">
<label>Password <input type="password" name="password" required></label>
<button type="submit">Confirm</button>
</form>
</main></body></html>"#;

/// Register the auth route table onto `table`.
pub fn register(table: &mut RouteTable) {
    table.get_action("/login", login_page).named("login");
    table.post_action("/login", login_submit);
    table.post_action("/logout", logout).named("logout");
    table
        .get_action("/register", register_page)
        .named("register");
    table.post_action("/register", register_submit);
    table
        .get_action("/confirm-password", confirm_page)
        .named("password.confirm");
    table.post_action("/confirm-password", confirm_submit);
}

/// GET /login — render the login form.
async fn login_page() -> Html<&'static str> {
    Html(LOGIN_HTML)
}

/// POST /login — unimplemented authentication flow.
async fn login_submit() -> Response {
    not_implemented("Login")
}

/// POST /logout — unimplemented session teardown.
async fn logout() -> Response {
    not_implemented("Logout")
}

/// GET /register — render the registration form.
async fn register_page() -> Html<&'static str> {
    Html(REGISTER_HTML)
}

/// POST /register — unimplemented registration flow.
async fn register_submit() -> Response {
    not_implemented("Registration")
}

/// GET /confirm-password — render the password-confirmation form.
async fn confirm_page() -> Html<&'static str> {
    Html(CONFIRM_HTML)
}

/// POST /confirm-password — unimplemented confirmation flow.
async fn confirm_submit() -> Response {
    not_implemented("Password confirmation")
}
