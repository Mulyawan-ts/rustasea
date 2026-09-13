//! Password-confirmation flow (AUTH-011).
//!
//! Split out of [`super`] (which is held at the 500-line cap) so the
//! `password.confirm` screen and its POST handler live beside each other. The
//! gate itself is enforced by `require_password_confirmed` in [`crate::routes`],
//! which reads the confirmation timestamp the session guard projects onto
//! [`AuthUser::password_confirmed_at`]; this module only *writes* that
//! timestamp after re-verifying the user's password.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::Extension;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};

use rustasea::auth::verify::{Argon2Verifier, PasswordVerifier};
use rustasea::auth::{AuthUser, SessionGuard};
use rustasea::http::AppState;

use crate::routes::helpers::{field, parse_form, see_other, user_provider};
use crate::routes::{session_id_from_headers, LOGIN_PATH};

use super::{fail_closed, field_error, AUTH_UNAVAILABLE, INVALID_CREDENTIALS};

/// Password-confirmation page markup (placeholder form).
const CONFIRM_HTML: &str = r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><title>Confirm password</title></head>
<body><main><h1>Confirm password</h1>
<form method="post" action="/confirm-password">
<label>Password <input type="password" name="password" required></label>
<button type="submit">Confirm</button>
</form>
</main></body></html>"#;

/// Where a successful confirmation sends the user.
///
/// The kit returns to the URL that triggered the gate via the session's
/// `url.intended` entry. RustaSea has no `intended` mechanism (nothing writes
/// one), so a fixed, sensible target is used instead: the only route gated by
/// `password.confirm` is `/settings/security`, which is exactly where a user
/// confirming their password wants to land. This is documented so a future
/// `intended` implementation can replace it deliberately.
const CONFIRM_TARGET: &str = "/settings/security";

/// GET /confirm-password — render the password-confirmation form.
pub(super) async fn confirm_page() -> Html<&'static str> {
    Html(CONFIRM_HTML)
}

/// POST /confirm-password — re-verify the password and stamp the session.
///
/// Authenticated-only: a request without a resolved principal is `302` →
/// [`LOGIN_PATH`] (the web-app convention the other gates use). The submitted
/// `password` is verified against the authenticated user's stored hash through
/// the async [`UserProvider`] seam plus [`Argon2Verifier`]; a mismatch answers
/// `422` with a `password` field error, byte-identical to the login failure so
/// nothing extra is disclosed. On success [`Guard::confirm_password`] writes the
/// confirmation timestamp onto the *current* session and the response is `303` →
/// [`CONFIRM_TARGET`]. A missing guard/session/email, an unresolvable record, or
/// a store failure fails closed with `500`.
///
/// [`UserProvider`]: rustasea::auth::UserProvider
/// [`Guard::confirm_password`]: rustasea::auth::Guard::confirm_password
pub(super) async fn confirm_submit(
    user: Option<Extension<AuthUser>>,
    Extension(state): Extension<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Authenticated-only. `Extension<AuthUser>` is inserted by the session
    // layer; its absence means "not logged in", so send the browser to login.
    let Some(Extension(user)) = user else {
        return found(LOGIN_PATH);
    };
    let Some(guard) = state.auth::<SessionGuard>() else {
        return fail_closed(AUTH_UNAVAILABLE);
    };
    let Some(session_id) = session_id_from_headers(&headers) else {
        return fail_closed(AUTH_UNAVAILABLE);
    };
    let Some(email) = user.email.as_deref() else {
        return fail_closed(AUTH_UNAVAILABLE);
    };

    let fields = parse_form(&body);
    let Some(password) = field(&fields, "password").filter(|value| !value.is_empty()) else {
        return field_error("password", "required", "The password field is required.");
    };

    // Resolve the stored hash for the authenticated identity, then verify.
    let provider = user_provider();
    let Ok(Some(record)) = provider.find_by_email(email).await else {
        return fail_closed(AUTH_UNAVAILABLE);
    };
    if !Argon2Verifier::new().verify(&record.password_hash, password) {
        return field_error("password", "AuthError::BadCredentials", INVALID_CREDENTIALS);
    }

    // Stamp the confirmation onto the live session; a store failure fails closed.
    if guard.confirm_password(&session_id).await.is_err() {
        return fail_closed(AUTH_UNAVAILABLE);
    }
    see_other(CONFIRM_TARGET)
}

/// Build the `302 Found` redirect to `location`.
///
/// Distinct from [`see_other`](super::helpers::see_other) (`303`): this is the
/// gate's redirect to the login screen, where the browser should simply GET the
/// login page (the request was not a successful form submission).
fn found(location: &str) -> Response {
    let value = HeaderValue::try_from(location).unwrap_or_else(|_| HeaderValue::from_static("/"));
    (StatusCode::FOUND, [(header::LOCATION, value)]).into_response()
}
