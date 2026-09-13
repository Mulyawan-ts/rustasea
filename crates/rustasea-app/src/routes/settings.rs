//! Settings routes — profile, password, and security management.
//!
//! `/settings` redirects to `/settings/profile`. The profile and password
//! pages render minimal placeholders; the password `PUT` answers
//! `501 Not Implemented`. `/settings/security` is gated by the
//! `password.confirm` middleware id registered in [`super`].

use axum::response::{Html, Response};

use rustasea::router::Router as RouteTable;

use super::{not_implemented, PASSWORD_CONFIRM};

/// Profile settings page markup (placeholder).
const PROFILE_HTML: &str = r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><title>Profile settings</title></head>
<body><main><h1>Profile settings</h1>
<p>Update your name and email address here.</p>
</main></body></html>"#;

/// Password settings page markup (placeholder).
const PASSWORD_HTML: &str = r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><title>Password settings</title></head>
<body><main><h1>Password settings</h1>
<p>Change your account password here.</p>
</main></body></html>"#;

/// Security settings page markup (placeholder, gated).
const SECURITY_HTML: &str = r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><title>Security settings</title></head>
<body><main><h1>Security settings</h1>
<p>Two-factor authentication and sessions appear here.</p>
</main></body></html>"#;

/// Register the settings route table onto `table`.
///
/// `/settings/security` is registered inside a [`RouteTable::group`] because
/// [`RouteTable::middleware`] is sticky — its pending middleware applies to
/// every subsequent route on the same table. The group scopes the
/// `password.confirm` gate so it cannot leak onto the console table.
pub fn register(table: &mut RouteTable) {
    table.redirect("/settings", "/settings/profile");
    table
        .get_action("/settings/profile", profile_page)
        .named("profile.edit");
    table
        .get_action("/settings/password", password_page)
        .named("password.edit");
    table.put_action("/settings/password", password_update);
    table.group(|group| {
        group
            .middleware(PASSWORD_CONFIRM)
            .get_action("/settings/security", security_page)
            .named("security.edit");
    });
}

/// GET /settings/profile — render the profile form.
async fn profile_page() -> Html<&'static str> {
    Html(PROFILE_HTML)
}

/// GET /settings/password — render the password form.
async fn password_page() -> Html<&'static str> {
    Html(PASSWORD_HTML)
}

/// PUT /settings/password — unimplemented password update.
async fn password_update() -> Response {
    not_implemented("Password update")
}

/// GET /settings/security — render the security page behind the gate.
async fn security_page() -> Html<&'static str> {
    Html(SECURITY_HTML)
}
